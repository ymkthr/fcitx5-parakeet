/*
 * SPDX-License-Identifier: MIT
 *
 * fcitx5 module that dictates through parakeetd from any input method.
 *
 * The module watches key events ahead of the active input method and claims
 * only the trigger/cancel keys, so the user keeps their keyboard input method
 * (and can edit the transcript right away) while dictating.
 */
#ifndef FCITX5_PARAKEET_PARAKEET_H
#define FCITX5_PARAKEET_PARAKEET_H

#include <chrono>
#include <cstdint>
#include <memory>
#include <string>
#include <vector>

#include <fcitx-config/configuration.h>
#include <fcitx-config/iniparser.h>
#include <fcitx-config/option.h>
#include <fcitx-utils/event.h>
#include <fcitx-utils/i18n.h>
#include <fcitx-utils/key.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addoninstance.h>
#include <fcitx/addonmanager.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputcontextproperty.h>
#include <fcitx/instance.h>

#include "client.h"

namespace fcitx {

FCITX_CONFIGURATION(
    ParakeetConfig, KeyListOption triggerKey{this,
                                             "TriggerKey",
                                             _("Trigger key"),
                                             {Key("Menu")},
                                             KeyListConstrain({KeyConstrainFlag::AllowModifierLess})};
    KeyListOption cancelKey{this,
                            "CancelKey",
                            _("Cancel key (while recording)"),
                            {Key("Escape")},
                            KeyListConstrain({KeyConstrainFlag::AllowModifierLess})};
    Option<int, IntConstrain> tapThreshold{
        this, "TapThresholdMs",
        _("Tap threshold (ms): a shorter press locks the recording, a longer press records while held"), 250,
        IntConstrain(0, 2000)};
    Option<std::string> language{this, "Language", _("Language sent to parakeetd (auto, ja or en)"), "auto"};
    Option<std::string> socketPath{this, "SocketPath",
                                   _("parakeetd socket (empty: $XDG_RUNTIME_DIR/parakeetd.sock)"), ""};
    Option<bool> showStatus{this, "ShowStatus", _("Show recording status near the cursor"), true};);

struct ParakeetState : public InputContextProperty {
    // The trigger is held down (independent of whether the daemon accepted
    // the capture).
    bool armed = false;
    bool recording = false;
    // Wait for capture confirmation before showing the recording cue.
    bool live = false;
    // A tap locked the recording: it runs until the next trigger press.
    bool locked = false;
    // The press that ended a locked recording was consumed; hide its release too.
    bool swallowRelease = false;
    int pendingResults = 0;
    // Bumped per capture so late START replies cannot touch a newer session.
    uint64_t session = 0;
    std::chrono::steady_clock::time_point pressedAt;
};

class ParakeetModule final : public AddonInstance {
  public:
    explicit ParakeetModule(Instance *instance);

    const Configuration *getConfig() const override { return &config_; }
    void setConfig(const RawConfig &raw) override;
    void reloadConfig() override;

  private:
    void applyConfig();
    void onKeyEvent(KeyEvent &event);
    bool isTriggerRelease(const Key &key) const;

    void startRecording(InputContext *ic, ParakeetState *state);
    void stopRecording(InputContext *ic, ParakeetState *state);
    void cancelRecording(InputContext *ic, ParakeetState *state);

    void updateStatus(InputContext *ic, const ParakeetState *state);
    void failStatus(InputContext *ic, const std::string &message);
    void showAux(InputContext *ic, const std::string &text);

    Instance *instance_;
    ParakeetConfig config_;
    FactoryFor<ParakeetState> factory_;
    std::unique_ptr<ParakeetClient> client_;
    std::unique_ptr<EventSourceTime> failTimer_;
    std::vector<std::unique_ptr<HandlerTableEntry<EventHandler>>> watchers_;
};

class ParakeetModuleFactory : public AddonFactory {
  public:
    AddonInstance *create(AddonManager *manager) override { return new ParakeetModule(manager->instance()); }
};

} // namespace fcitx

#endif // FCITX5_PARAKEET_PARAKEET_H
