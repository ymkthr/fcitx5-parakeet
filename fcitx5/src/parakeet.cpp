/*
 * SPDX-License-Identifier: MIT
 */
#include "parakeet.h"

#include <algorithm>
#include <utility>

#include <fcitx-utils/capabilityflags.h>
#include <fcitx-utils/log.h>
#include <fcitx-utils/utf8.h>
#include <fcitx/event.h>
#include <fcitx/inputcontextmanager.h>
#include <fcitx/inputpanel.h>
#include <fcitx/surroundingtext.h>
#include <fcitx/text.h>
#include <fcitx/userinterface.h>

namespace fcitx {

namespace {

FCITX_DEFINE_LOG_CATEGORY(parakeet_log, "parakeet");
#define PK_DEBUG() FCITX_LOGC(parakeet_log, Debug)
#define PK_WARN() FCITX_LOGC(parakeet_log, Warn)

constexpr const char *kConfigFile = "conf/parakeet.conf";
constexpr uint64_t kUsec = 1000 * 1000;
constexpr uint64_t kFailStatusUsec = 2500 * 1000;
// parakeetd answers START once samples flow (RECORDER_START_TIMEOUT = 3 s).
constexpr uint64_t kStartTimeoutUsec = 10 * kUsec;
// Transcription of a 120 s capture stays well under this on CPU.
constexpr uint64_t kStopTimeoutUsec = 60 * kUsec;
constexpr uint64_t kCancelTimeoutUsec = 5 * kUsec;
// Context length the jinen-v2 model card uses; parakeetd truncates to the same.
constexpr size_t kContextChars = 64;
// Keeps the live transcript to one short line near the cursor.
constexpr size_t kPartialChars = 40;

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

/// Text before the cursor, sent with STOP so parakeetd can pick homophones
/// that fit what is already written. Kept on one protocol line.
std::string contextBeforeCursor(InputContext *ic) {
    if (!ic->capabilityFlags().test(CapabilityFlag::SurroundingText)) {
        return {};
    }
    const auto &surrounding = ic->surroundingText();
    if (!surrounding.isValid()) {
        return {};
    }
    const std::string &text = surrounding.text();
    const size_t cursor = surrounding.cursor();
    const size_t length = utf8::lengthValidated(text);
    if (length == utf8::INVALID_LENGTH || cursor > length) {
        return {};
    }
    const size_t first = cursor > kContextChars ? cursor - kContextChars : 0;
    const auto begin = utf8::nextNChar(text.begin(), first);
    std::string context(begin, utf8::nextNChar(begin, cursor - first));
    std::replace_if(
        context.begin(), context.end(), [](char c) { return c == '\r' || c == '\n' || c == '\t'; }, ' ');
    return context;
}

/// The active input method is composing (e.g. kana awaiting conversion);
/// dictating into the middle of that would corrupt both.
bool composing(InputContext *ic) {
    const auto &panel = ic->inputPanel();
    return panel.clientPreedit().size() > 0 || panel.preedit().size() > 0;
}

/// The status cue followed by the end of the live transcript, if any.
std::string withPartial(std::string cue, const std::string &partial) {
    const size_t length = utf8::lengthValidated(partial);
    if (partial.empty() || length == utf8::INVALID_LENGTH) {
        return cue;
    }
    cue += ' ';
    if (length > kPartialChars) {
        cue += "…";
        cue.append(utf8::nextNChar(partial.begin(), length - kPartialChars), partial.end());
    } else {
        cue += partial;
    }
    return cue;
}

} // namespace

ParakeetModule::ParakeetModule(Instance *instance)
    : instance_(instance), factory_([](InputContext &) { return new ParakeetState; }) {
    registerDomain("fcitx5-parakeet", FCITX_INSTALL_LOCALEDIR);
    instance_->inputContextManager().registerProperty("parakeetState", &factory_);
    client_ = std::make_unique<ParakeetClient>(instance_->eventLoop(), defaultParakeetSocketPath());
    reloadConfig();

    watchers_.emplace_back(
        instance_->watchEvent(EventType::InputContextKeyEvent, EventWatcherPhase::PreInputMethod,
                              [this](Event &event) { onKeyEvent(static_cast<KeyEvent &>(event)); }));
    watchers_.emplace_back(instance_->watchEvent(
        EventType::InputContextFocusOut, EventWatcherPhase::Default, [this](Event &event) {
            // A capture nobody can finish is dropped. Pending transcripts
            // still commit when they arrive.
            auto *ic = static_cast<InputContextEvent &>(event).inputContext();
            auto *state = ic->propertyFor(&factory_);
            if (state->recording) {
                cancelRecording(ic, state);
            }
            state->armed = false;
            state->locked = false;
            state->swallowRelease = false;
        }));
}

void ParakeetModule::applyConfig() {
    const std::string &path = *config_.socketPath;
    client_->setSocketPath(path.empty() ? defaultParakeetSocketPath() : path);
}

void ParakeetModule::setConfig(const RawConfig &raw) {
    config_.load(raw, true);
    safeSaveAsIni(config_, kConfigFile);
    applyConfig();
}

void ParakeetModule::reloadConfig() {
    readAsIni(config_, kConfigFile);
    applyConfig();
}

bool ParakeetModule::isTriggerRelease(const Key &key) const {
    // Modifiers may be released before the main key (Super+space -> space),
    // so the release only has to match the key symbol.
    return std::any_of(config_.triggerKey->begin(), config_.triggerKey->end(), [&](const Key &trigger) {
        return trigger.sym() != FcitxKey_None ? trigger.sym() == key.sym()
                                              : trigger.code() != 0 && trigger.code() == key.code();
    });
}

void ParakeetModule::onKeyEvent(KeyEvent &event) {
    auto *ic = event.inputContext();
    auto *state = ic->propertyFor(&factory_);
    const Key key = event.key();

    if (event.isRelease()) {
        if (!isTriggerRelease(key)) {
            return;
        }
        if (state->swallowRelease) {
            state->swallowRelease = false;
            return event.filterAndAccept();
        }
        if (!state->armed) {
            return;
        }
        state->armed = false;
        const auto held = std::chrono::steady_clock::now() - state->pressedAt;
        if (state->recording) {
            if (held < std::chrono::milliseconds(*config_.tapThreshold)) {
                // A tap locks the recording until the next press.
                state->locked = true;
            } else {
                stopRecording(ic, state);
            }
        }
        return event.filterAndAccept();
    }

    if (key.checkKeyList(*config_.triggerKey)) {
        // Auto-repeat delivers more presses while the key is held; only the
        // first one counts.
        if (state->armed || state->swallowRelease) {
            return event.filterAndAccept();
        }
        if (state->recording && state->locked) {
            state->locked = false;
            state->swallowRelease = true;
            stopRecording(ic, state);
            return event.filterAndAccept();
        }
        if (composing(ic)) {
            PK_DEBUG() << "trigger ignored: input method is composing";
            state->swallowRelease = true;
            return event.filterAndAccept();
        }
        state->armed = true;
        state->pressedAt = std::chrono::steady_clock::now();
        startRecording(ic, state);
        return event.filterAndAccept();
    }

    if (state->recording && key.checkKeyList(*config_.cancelKey)) {
        cancelRecording(ic, state);
        state->locked = false;
        return event.filterAndAccept();
    }
}

void ParakeetModule::startRecording(InputContext *ic, ParakeetState *state) {
    state->recording = true;
    state->live = false;
    state->locked = false;
    state->partial.clear();
    const uint64_t session = ++state->session;
    updateStatus(ic, state);

    auto ref = ic->watch();
    state->startRequest = client_->request(
        "START", *config_.language,
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
            client_->unsubscribe(state->startRequest);
            state->recording = false;
            state->locked = false;
            failStatus(ic, payload);
        },
        kStartTimeoutUsec,
        [this, ref, session](std::string text) {
            auto *ic = ref.get();
            if (!ic) {
                return;
            }
            auto *state = ic->propertyFor(&factory_);
            if (state->session != session || !state->recording) {
                return;
            }
            state->partial = std::move(text);
            updateStatus(ic, state);
        });
    if (state->startRequest == 0) {
        state->recording = false;
        failStatus(ic, _("parakeetd is not running"));
    }
}

void ParakeetModule::stopRecording(InputContext *ic, ParakeetState *state) {
    state->recording = false;
    client_->unsubscribe(state->startRequest);
    ++state->pendingResults;
    updateStatus(ic, state);

    auto ref = ic->watch();
    const uint64_t sent = client_->request(
        "STOP", contextBeforeCursor(ic),
        [this, ref](bool ok, std::string payload) {
            auto *ic = ref.get();
            if (!ic) {
                return;
            }
            auto *state = ic->propertyFor(&factory_);
            state->pendingResults = std::max(0, state->pendingResults - 1);
            if (state->pendingResults == 0 && !state->recording) {
                state->partial.clear();
            }
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
    if (sent == 0) {
        state->pendingResults = std::max(0, state->pendingResults - 1);
        failStatus(ic, _("parakeetd is not running"));
    }
}

void ParakeetModule::cancelRecording(InputContext *ic, ParakeetState *state) {
    state->recording = false;
    ++state->session;
    client_->unsubscribe(state->startRequest);
    state->partial.clear();
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

void ParakeetModule::showAux(InputContext *ic, const std::string &text) {
    // Only the aux line is ours; the active input method owns the rest of
    // the panel (preedit, candidates), so it is left untouched.
    ic->inputPanel().setAuxUp(Text(text));
    ic->updateUserInterface(UserInterfaceComponent::InputPanel);
}

void ParakeetModule::updateStatus(InputContext *ic, const ParakeetState *state) {
    if (!*config_.showStatus) {
        showAux(ic, {});
        return;
    }
    if (state->recording) {
        showAux(ic, state->live ? withPartial("🎙️", state->partial) : "");
    } else if (state->pendingResults > 0) {
        showAux(ic, withPartial("…", state->partial));
    } else {
        showAux(ic, {});
    }
}

void ParakeetModule::failStatus(InputContext *ic, const std::string &message) {
    PK_WARN() << message;
    showAux(ic, std::string(_("Parakeet: ")) + message);
    auto ref = ic->watch();
    failTimer_ = instance_->eventLoop().addTimeEvent(CLOCK_MONOTONIC, now(CLOCK_MONOTONIC) + kFailStatusUsec,
                                                     0, [this, ref](EventSourceTime *source, uint64_t) {
                                                         source->setEnabled(false);
                                                         if (auto *ic = ref.get()) {
                                                             updateStatus(ic, ic->propertyFor(&factory_));
                                                         }
                                                         return true;
                                                     });
}

} // namespace fcitx

FCITX_ADDON_FACTORY_V2(parakeet, fcitx::ParakeetModuleFactory);
