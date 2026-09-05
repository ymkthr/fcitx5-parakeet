/*
 * SPDX-License-Identifier: MIT
 */
#include "parakeet.h"

#include <algorithm>
#include <utility>

#include <fcitx-utils/log.h>
#include <fcitx/inputmethodentry.h>
#include <fcitx/inputpanel.h>
#include <fcitx/surroundingtext.h>
#include <fcitx/text.h>
#include <fcitx/userinterface.h>
#include <fcitx-utils/capabilityflags.h>
#include <fcitx-utils/utf8.h>

namespace fcitx {

namespace {

FCITX_DEFINE_LOG_CATEGORY(parakeet_log, "parakeet");
#define PK_DEBUG() FCITX_LOGC(parakeet_log, Debug)
#define PK_WARN() FCITX_LOGC(parakeet_log, Warn)

constexpr const char *kConfigFile = "conf/parakeet.conf";
constexpr uint64_t kUsec = 1000 * 1000;
constexpr uint64_t kFailStatusUsec = 2500 * 1000;
// Model load can take a few seconds per language on a cold daemon start.
constexpr uint64_t kLoadTimeoutUsec = 60 * kUsec;
// parakeetd answers START once samples flow (RECORDER_START_TIMEOUT = 3 s).
constexpr uint64_t kStartTimeoutUsec = 10 * kUsec;
// Transcription of a 120 s capture stays well under this on CPU.
constexpr uint64_t kStopTimeoutUsec = 60 * kUsec;
constexpr uint64_t kCancelTimeoutUsec = 5 * kUsec;

/// English dictation appended to existing text needs a word separator; the
/// models return transcripts without a leading space.
bool needsLeadingSpace(InputContext *ic) {
    if (!ic->capabilityFlags().test(CapabilityFlag::SurroundingText)) {
        return false;
    }
    const auto &surrounding = ic->surroundingText();
    if (!surrounding.isValid() || surrounding.cursor() == 0) {
        return false;
    }
    const std::string &text = surrounding.text();
    const auto offset = utf8::ncharByteLength(text.begin(), surrounding.cursor());
    if (offset <= 0 || static_cast<size_t>(offset) > text.size()) {
        return false;
    }
    const char before = text[static_cast<size_t>(offset) - 1];
    return before != ' ' && before != '\n' && before != '\t';
}

} // namespace

ParakeetEngine::ParakeetEngine(Instance *instance)
    : instance_(instance), factory_([](InputContext &) { return new ParakeetState; }) {
    registerDomain("fcitx5-parakeet", FCITX_INSTALL_LOCALEDIR);
    instance_->inputContextManager().registerProperty("parakeetState", &factory_);
    client_ = std::make_unique<ParakeetClient>(instance_->eventLoop(), defaultParakeetSocketPath());
    reloadConfig();
}

std::string ParakeetEngine::langOf(const InputMethodEntry &entry) {
    // parakeet-ja / parakeet-en / parakeet-auto -> ja / en / auto: the IM
    // name selects the daemon language, so "auto" can still carry LangCode=ja
    // for fcitx5's own language grouping.
    static constexpr std::string_view kPrefix = "parakeet-";
    const std::string &name = entry.uniqueName();
    if (name.starts_with(kPrefix)) {
        return name.substr(kPrefix.size());
    }
    // "en_US" -> "en" for entries added by hand with another name.
    const std::string &code = entry.languageCode();
    return code.substr(0, code.find_first_of("_-"));
}

void ParakeetEngine::applyConfig() {
    const std::string &path = *config_.socketPath;
    client_->setSocketPath(path.empty() ? defaultParakeetSocketPath() : path);
}

void ParakeetEngine::setConfig(const RawConfig &raw) {
    config_.load(raw, true);
    safeSaveAsIni(config_, kConfigFile);
    applyConfig();
}

void ParakeetEngine::reloadConfig() {
    readAsIni(config_, kConfigFile);
    applyConfig();
}

void ParakeetEngine::activate(const InputMethodEntry &entry, InputContextEvent &event) {
    // Connecting here spawns parakeetd through socket activation and warms the
    // model, so the first press is not delayed by a cold start.
    auto *ic = event.inputContext();
    const std::string lang = langOf(entry);
    auto ref = ic->watch();
    const bool sent = client_->request(
        "LOAD", lang,
        [this, ref](bool ok, std::string payload) {
            if (ok) {
                return;
            }
            if (auto *ic = ref.get()) {
                failStatus(ic, payload);
            }
        },
        kLoadTimeoutUsec);
    if (!sent) {
        failStatus(ic, _("parakeetd is not running"));
    }
}

void ParakeetEngine::deactivate(const InputMethodEntry & /*entry*/, InputContextEvent &event) {
    // Focus left or the user switched input method: a capture nobody can
    // finish is dropped. Pending transcripts still commit when they arrive.
    auto *ic = event.inputContext();
    auto *state = ic->propertyFor(&factory_);
    if (state->recording) {
        cancelRecording(ic, state);
    }
    state->armed = false;
    state->swallowRelease = false;
    updateStatus(ic, state);
}

void ParakeetEngine::reset(const InputMethodEntry & /*entry*/, InputContextEvent &event) {
    // Toolkits call reset() on cursor moves and buffer edits, which can
    // happen while the trigger is held, so a capture in progress survives.
    auto *ic = event.inputContext();
    updateStatus(ic, ic->propertyFor(&factory_));
}

std::string ParakeetEngine::subMode(const InputMethodEntry & /*entry*/, InputContext &ic) {
    const auto *state = ic.propertyFor(&factory_);
    if (state->recording) {
        return _("Listening");
    }
    if (state->pendingResults > 0) {
        return _("Transcribing");
    }
    return {};
}

std::string ParakeetEngine::subModeIconImpl(const InputMethodEntry & /*entry*/, InputContext &ic) {
    // Panel indicators (kimpanel, tray) ask Instance::inputMethodIcon, which
    // prefers this over the entry icon: red while capturing, amber while the
    // daemon transcribes, the language icon otherwise.
    const auto *state = ic.propertyFor(&factory_);
    if (state->recording && state->live) {
        return "fcitx-parakeet-recording";
    }
    if (state->pendingResults > 0) {
        return "fcitx-parakeet-busy";
    }
    return {};
}

void ParakeetEngine::keyEvent(const InputMethodEntry &entry, KeyEvent &event) {
    auto *ic = event.inputContext();
    auto *state = ic->propertyFor(&factory_);
    const Key key = event.key();
    const bool trigger = key.checkKeyList(*config_.triggerKey);

    if (event.isRelease()) {
        if (!trigger) {
            return;
        }
        if (state->armed) {
            state->armed = false;
            const auto held = std::chrono::steady_clock::now() - state->pressedAt;
            if (held < std::chrono::milliseconds(*config_.tapThreshold)) {
                // A quick tap was not dictation: behave like the plain key.
                if (state->recording) {
                    cancelRecording(ic, state);
                }
                typeKey(ic, key);
            } else if (state->recording) {
                stopRecording(ic, state);
            }
            return event.filterAndAccept();
        }
        if (state->swallowRelease) {
            state->swallowRelease = false;
            return event.filterAndAccept();
        }
        return;
    }

    if (trigger) {
        if (*config_.mode == TriggerMode::PushToTalk) {
            // Auto-repeat delivers more presses while the key is held; only
            // the first one starts the capture.
            if (!state->armed) {
                state->armed = true;
                state->pressedAt = std::chrono::steady_clock::now();
                startRecording(ic, state, langOf(entry));
            }
        } else {
            state->swallowRelease = true;
            if (state->recording) {
                stopRecording(ic, state);
            } else {
                startRecording(ic, state, langOf(entry));
            }
        }
        return event.filterAndAccept();
    }

    if (state->recording && key.checkKeyList(*config_.cancelKey)) {
        cancelRecording(ic, state);
        return event.filterAndAccept();
    }
}

void ParakeetEngine::typeKey(InputContext *ic, const Key &key) {
    if (!key.hasModifier()) {
        if (std::string text = Key::keySymToUTF8(key.sym()); !text.empty()) {
            ic->commitString(text);
            return;
        }
    }
    ic->forwardKey(key, false);
    ic->forwardKey(key, true);
}

void ParakeetEngine::startRecording(InputContext *ic, ParakeetState *state, const std::string &lang) {
    state->recording = true;
    state->live = false;
    const uint64_t session = ++state->session;
    updateStatus(ic, state);

    auto ref = ic->watch();
    const bool sent = client_->request(
        "START", lang,
        [this, ref, session](bool ok, std::string payload) {
            auto *ic = ref.get();
            if (!ic) {
                return;
            }
            auto *state = ic->propertyFor(&factory_);
            if (state->session != session || !state->recording) {
                return;
            }
            PK_DEBUG() << "START reply session=" << session << " ok=" << ok;
            if (ok) {
                state->live = true;
                updateStatus(ic, state);
                return;
            }
            state->recording = false;
            failStatus(ic, payload);
        },
        kStartTimeoutUsec);
    if (!sent) {
        state->recording = false;
        failStatus(ic, _("parakeetd is not running"));
    }
}

void ParakeetEngine::stopRecording(InputContext *ic, ParakeetState *state) {
    state->recording = false;
    ++state->pendingResults;
    updateStatus(ic, state);

    auto ref = ic->watch();
    const bool sent = client_->request(
        "STOP", "",
        [this, ref](bool ok, std::string payload) {
            auto *ic = ref.get();
            if (!ic) {
                return;
            }
            auto *state = ic->propertyFor(&factory_);
            state->pendingResults = std::max(0, state->pendingResults - 1);
            if (!ok) {
                failStatus(ic, payload);
                return;
            }
            // payload: "<lang> <text>", text may be empty.
            const size_t space = payload.find(' ');
            const std::string_view lang = std::string_view(payload).substr(0, space);
            std::string text = space == std::string::npos ? std::string() : payload.substr(space + 1);
            updateStatus(ic, state);
            if (text.empty()) {
                return;
            }
            if (lang == "en" && needsLeadingSpace(ic)) {
                text.insert(text.begin(), ' ');
            }
            PK_DEBUG() << "commit " << text.size() << " bytes";
            ic->commitString(text);
        },
        kStopTimeoutUsec);
    if (!sent) {
        state->pendingResults = std::max(0, state->pendingResults - 1);
        failStatus(ic, _("parakeetd is not running"));
    }
}

void ParakeetEngine::cancelRecording(InputContext *ic, ParakeetState *state) {
    state->recording = false;
    ++state->session;
    updateStatus(ic, state);
    client_->request(
        "CANCEL", "",
        [](bool ok, std::string payload) {
            if (!ok) {
                PK_WARN() << "cancel failed: " << payload;
            }
        },
        kCancelTimeoutUsec);
}

void ParakeetEngine::showAux(InputContext *ic, const std::string &text) {
    auto &panel = ic->inputPanel();
    panel.reset();
    if (!text.empty()) {
        panel.setAuxUp(Text(text));
    }
    ic->updateUserInterface(UserInterfaceComponent::InputPanel);
    // Re-query subMode / subModeIcon so the indicator follows the state.
    ic->updateUserInterface(UserInterfaceComponent::StatusArea);
}

void ParakeetEngine::updateStatus(InputContext *ic, const ParakeetState *state) {
    if (!*config_.showStatus) {
        showAux(ic, {});
        return;
    }
    // Language-neutral glyphs: the aux popup cannot hold images, so the
    // microphone emoji stands in for "recording" and an ellipsis for
    // "transcribing". Both wait for the daemon's confirmation, so their
    // appearance is the cue that speech is being captured.
    if (state->recording) {
        showAux(ic, state->live ? "🎙️" : "");
    } else if (state->pendingResults > 0) {
        showAux(ic, "…");
    } else {
        showAux(ic, {});
    }
}

void ParakeetEngine::failStatus(InputContext *ic, const std::string &message) {
    PK_WARN() << message;
    showAux(ic, std::string(_("Parakeet: ")) + message);
    auto ref = ic->watch();
    failTimer_ = instance_->eventLoop().addTimeEvent(
        CLOCK_MONOTONIC, now(CLOCK_MONOTONIC) + kFailStatusUsec, 0,
        [this, ref](EventSourceTime *source, uint64_t) {
            source->setEnabled(false);
            if (auto *ic = ref.get()) {
                updateStatus(ic, ic->propertyFor(&factory_));
            }
            return true;
        });
}

} // namespace fcitx

FCITX_ADDON_FACTORY_V2(parakeet, fcitx::ParakeetEngineFactory);
