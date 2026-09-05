/*
 * SPDX-License-Identifier: MIT
 *
 * fcitx5 input method engine that dictates through parakeetd.
 *
 * The engine deliberately touches only the trigger/cancel keys; every other
 * key event passes straight to the application so the "voice" input method
 * still types like a plain keyboard layout.
 */
#ifndef FCITX5_PARAKEET_PARAKEET_H
#define FCITX5_PARAKEET_PARAKEET_H

#include <chrono>
#include <cstdint>
#include <memory>
#include <string>

#include <fcitx-config/configuration.h>
#include <fcitx-config/enum.h>
#include <fcitx-config/iniparser.h>
#include <fcitx-config/option.h>
#include <fcitx-utils/event.h>
#include <fcitx-utils/i18n.h>
#include <fcitx-utils/key.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addonmanager.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputcontextproperty.h>
#include <fcitx/inputmethodengine.h>
#include <fcitx/instance.h>

#include "client.h"

namespace fcitx {

enum class TriggerMode { PushToTalk, Toggle };
FCITX_CONFIG_ENUM_NAME_WITH_I18N(TriggerMode, N_("Push to talk"), N_("Toggle"));

FCITX_CONFIGURATION(
    ParakeetConfig,
    OptionWithAnnotation<TriggerMode, TriggerModeI18NAnnotation> mode{this, "Mode", _("Trigger mode"),
                                                                       TriggerMode::PushToTalk};
    KeyListOption triggerKey{this, "TriggerKey", _("Trigger key"), {Key("space")},
                             KeyListConstrain({KeyConstrainFlag::AllowModifierLess})};
    KeyListOption cancelKey{this, "CancelKey", _("Cancel key (while recording)"), {Key("Escape")},
                            KeyListConstrain({KeyConstrainFlag::AllowModifierLess})};
    Option<int, IntConstrain> tapThreshold{
        this, "TapThresholdMs", _("Push to talk: a press shorter than this (ms) types the key instead"), 250,
        IntConstrain(0, 2000)};
    Option<std::string> socketPath{this, "SocketPath", _("parakeetd socket (empty: $XDG_RUNTIME_DIR/parakeetd.sock)"),
                                   ""};
    Option<bool> showStatus{this, "ShowStatus", _("Show recording status near the cursor"), true};);

struct ParakeetState : public InputContextProperty {
    // Push to talk: the trigger is held down (independent of whether the
    // daemon accepted the capture, so a tap still types the key).
    bool armed = false;
    bool recording = false;
    // parakeetd confirmed samples are flowing; the 🎙️ / red icon wait for this
    // so they double as the "start speaking" cue (~150 ms after the press).
    bool live = false;
    int pendingResults = 0;
    // Toggle mode: the trigger press was consumed, so hide its release too.
    bool swallowRelease = false;
    // Bumped per capture so late START replies cannot touch a newer session.
    uint64_t session = 0;
    std::chrono::steady_clock::time_point pressedAt;
};

class ParakeetEngine final : public InputMethodEngineV2 {
public:
    explicit ParakeetEngine(Instance *instance);

    void keyEvent(const InputMethodEntry &entry, KeyEvent &event) override;
    void activate(const InputMethodEntry &entry, InputContextEvent &event) override;
    void deactivate(const InputMethodEntry &entry, InputContextEvent &event) override;
    void reset(const InputMethodEntry &entry, InputContextEvent &event) override;
    std::string subMode(const InputMethodEntry &entry, InputContext &ic) override;
    std::string subModeIconImpl(const InputMethodEntry &entry, InputContext &ic) override;

    const Configuration *getConfig() const override { return &config_; }
    void setConfig(const RawConfig &raw) override;
    void reloadConfig() override;

private:
    static std::string langOf(const InputMethodEntry &entry);
    void applyConfig();

    void startRecording(InputContext *ic, ParakeetState *state, const std::string &lang);
    void stopRecording(InputContext *ic, ParakeetState *state);
    void cancelRecording(InputContext *ic, ParakeetState *state);
    void typeKey(InputContext *ic, const Key &key);

    void updateStatus(InputContext *ic, const ParakeetState *state);
    void failStatus(InputContext *ic, const std::string &message);
    void showAux(InputContext *ic, const std::string &text);

    Instance *instance_;
    ParakeetConfig config_;
    FactoryFor<ParakeetState> factory_;
    std::unique_ptr<ParakeetClient> client_;
    std::unique_ptr<EventSourceTime> failTimer_;
};

class ParakeetEngineFactory : public AddonFactory {
public:
    AddonInstance *create(AddonManager *manager) override { return new ParakeetEngine(manager->instance()); }
};

} // namespace fcitx

#endif // FCITX5_PARAKEET_PARAKEET_H
