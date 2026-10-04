/*
 * SPDX-License-Identifier: MIT
 *
 * Drives the voiceja module through fcitx5's test frontend against a live
 * voice-jad while the plain keyboard input method is active. Verifies:
 *   1. unrelated keys are not swallowed,
 *   2. holding the trigger dictates: the live transcript shows in the aux
 *      line while held, and the transcript is committed on release,
 *   3. a tap locks the recording until the next press,
 *   4. focus leaving mid-dictation keeps the transcript, committed once the
 *      input context has focus again.
 *
 * Environment:
 *   VOICE_JA_TEST_SOCKET  voice-jad socket (unset -> test skipped, exit 77)
 *   VOICE_JA_TEST_WAV     wav played into VOICE_JA_TEST_SINK while holding
 *   VOICE_JA_TEST_SINK    PipeWire sink whose monitor voice-jad captures
 *   VOICE_JA_TEST_EXPECT  exact transcript to require (optional; any commit otherwise)
 *   VOICE_JA_TEST_HOLD_SEC seconds to hold the trigger while the wav plays (default 8)
 */
#include <chrono>
#include <cstdlib>
#include <functional>
#include <memory>
#include <string>
#include <vector>

#include <spawn.h>
#include <sys/wait.h>

#include <fcitx-config/rawconfig.h>
#include <fcitx-utils/event.h>
#include <fcitx-utils/eventdispatcher.h>
#include <fcitx-utils/log.h>
#include <fcitx-utils/testing.h>
#include <fcitx/addonmanager.h>
#include <fcitx/event.h>
#include <fcitx/inputcontextmanager.h>
#include <fcitx/inputmethodmanager.h>
#include <fcitx/inputpanel.h>
#include <fcitx/instance.h>
#include <testfrontend_public.h>

using namespace fcitx;

extern char **environ;

namespace {

constexpr uint64_t kUsec = 1000 * 1000;
const Key kTrigger("Menu");

std::string envOr(const char *name, const char *fallback) {
    const char *value = std::getenv(name);
    return value ? value : fallback;
}

pid_t play(const std::string &sink, const std::string &wav) {
    const char *argv[] = {"pw-play", "--target", sink.c_str(), wav.c_str(), nullptr};
    pid_t pid = 0;
    if (posix_spawnp(&pid, "pw-play", nullptr, nullptr, const_cast<char **>(argv), environ) != 0) {
        FCITX_FATAL() << "cannot spawn pw-play";
    }
    return pid;
}

} // namespace

int main() {
    const char *socketPath = std::getenv("VOICE_JA_TEST_SOCKET");
    if (!socketPath) {
        FCITX_INFO() << "VOICE_JA_TEST_SOCKET not set; skipping";
        return 77;
    }
    const std::string wav = envOr("VOICE_JA_TEST_WAV", "");
    const std::string sink = envOr("VOICE_JA_TEST_SINK", "");
    // Exact-match check is optional: ASR output for some clips is not stable
    // across capture offsets, but a played wav must always produce a commit.
    const std::string expect = envOr("VOICE_JA_TEST_EXPECT", "");
    const bool expectCommit = !wav.empty() && !sink.empty();
    const auto dictateUsec = static_cast<uint64_t>(std::stod(envOr("VOICE_JA_TEST_HOLD_SEC", "8")) * kUsec);

    setupTestingEnvironment(TESTING_BINARY_DIR, {"src"}, {"test"});
    char arg0[] = "testvoiceja";
    char arg1[] = "--disable=all";
    char arg2[] = "--enable=testim,testfrontend,keyboard,voiceja";
    // VOICE_JA_TEST_VERBOSE=1 turns on the addon's own log categories.
    char arg3[] = "--verbose=voiceja=5,voiceja-client=5";
    char *argv[] = {arg0, arg1, arg2, arg3};
    const int argc = std::getenv("VOICE_JA_TEST_VERBOSE") ? 4 : 3;
    Instance instance(argc, argv);
    instance.addonManager().registerDefaultLoader(nullptr);

    EventDispatcher dispatcher;
    dispatcher.attach(&instance.eventLoop());

    ICUUID uuid{};
    AddonInstance *testfrontend = nullptr;
    InputContext *ic = nullptr;
    int commits = 0;
    std::string lastCommit;
    std::unique_ptr<HandlerTableEntry<EventHandler>> commitWatcher;
    std::vector<std::unique_ptr<EventSourceTime>> timers;
    pid_t player = 0;

    auto after = [&](uint64_t usec, std::function<void()> fn) {
        timers.push_back(
            instance.eventLoop().addTimeEvent(CLOCK_MONOTONIC, now(CLOCK_MONOTONIC) + usec, 0,
                                              [fn = std::move(fn)](EventSourceTime *source, uint64_t) {
                                                  source->setEnabled(false);
                                                  fn();
                                                  return true;
                                              }));
    };
    auto press = [&](const Key &key) { testfrontend->call<ITestFrontend::keyEvent>(uuid, key, false); };
    auto release = [&](const Key &key) { testfrontend->call<ITestFrontend::keyEvent>(uuid, key, true); };
    auto aux = [&]() { return ic->inputPanel().auxUp().toString(); };

    // 4. Focus leaves mid-dictation (a notification, a window switch): the
    //    recording ends without losing the speech, and the transcript waits
    //    until the input context has focus again.
    auto focusPhase = [&]() {
        const int before = commits;
        press(kTrigger);
        if (expectCommit) {
            waitpid(player, nullptr, 0);
            after(kUsec / 2, [&]() { player = play(sink, wav); });
        }
        after(expectCommit ? dictateUsec : kUsec, [&, before]() {
            ic->focusOut();
            after(15 * kUsec, [&, before]() {
                FCITX_ASSERT(commits == before) << "committed to an unfocused input context";
                ic->focusIn();
                FCITX_ASSERT(commits == before + (expectCommit ? 1 : 0)) << "transcript lost on focus change";
                instance.exit();
            });
        });
    };

    // 3. Tap: press and release at once, the recording keeps running (locked)
    //    and other keys still reach the application; the next press ends it.
    //    Silence is played, so nothing may be committed.
    auto tapPhase = [&]() {
        const int before = commits;
        press(kTrigger);
        release(kTrigger);
        FCITX_ASSERT(aux().empty()) << aux();
        after(kUsec + kUsec / 2, [&, before]() {
            FCITX_ASSERT(aux() == "🎙️") << "tap did not lock the recording: " << aux();
            FCITX_ASSERT(!testfrontend->call<ITestFrontend::sendKeyEvent>(uuid, Key("Control+c"), false));
            FCITX_ASSERT(aux() == "🎙️") << aux();
            FCITX_ASSERT(testfrontend->call<ITestFrontend::sendKeyEvent>(uuid, kTrigger, false));
            FCITX_ASSERT(testfrontend->call<ITestFrontend::sendKeyEvent>(uuid, kTrigger, true));
            after(3 * kUsec, [&, before]() {
                FCITX_ASSERT(commits == before) << "silence must not commit anything";
                FCITX_ASSERT(aux().empty()) << aux();
                focusPhase();
            });
        });
    };

    // 1. Unrelated keys pass through (not accepted by the module).
    // 2. Hold to dictate. Auto-repeat presses must be swallowed, and the
    //    toolkit-style reset() apps issue on cursor moves must not cancel.
    auto holdPhase = [&]() {
        FCITX_ASSERT(aux().empty()) << aux();
        FCITX_ASSERT(!testfrontend->call<ITestFrontend::sendKeyEvent>(uuid, Key("Control+c"), false));
        FCITX_ASSERT(!testfrontend->call<ITestFrontend::sendKeyEvent>(uuid, Key("Control+c"), true));
        FCITX_ASSERT(commits == 0);

        press(kTrigger);
        FCITX_ASSERT(testfrontend->call<ITestFrontend::sendKeyEvent>(uuid, kTrigger, false));
        ic->reset();
        // Nothing is shown until voice-jad confirms samples are flowing.
        FCITX_ASSERT(aux().empty()) << aux();

        uint64_t holdUsec = 1 * kUsec;
        if (expectCommit) {
            // Let the capture stream become live before playback starts, as a
            // person naturally pauses between pressing the key and speaking.
            after(kUsec / 2, [&]() { player = play(sink, wav); });
            holdUsec = dictateUsec;
        }
        after(holdUsec, [&]() {
            const std::string live = aux();
            FCITX_INFO() << "aux before release: " << live;
            FCITX_ASSERT(live.rfind("🎙️", 0) == 0) << "recording never went live: " << live;
            if (expectCommit) {
                FCITX_ASSERT(live.size() > std::string("🎙️ ").size()) << "no live transcript: " << live;
            }
            release(kTrigger);
        });
        after(holdUsec + 15 * kUsec, [&]() {
            if (expectCommit) {
                FCITX_ASSERT(commits == 1) << "transcript was never committed";
                FCITX_ASSERT(!lastCommit.empty());
                if (!expect.empty()) {
                    FCITX_ASSERT(lastCommit == expect) << lastCommit;
                }
            } else {
                FCITX_ASSERT(commits == 0) << "silence must not commit anything";
            }
            FCITX_ASSERT(aux().empty()) << aux();
            tapPhase();
        });
    };

    dispatcher.schedule([&]() {
        testfrontend = instance.addonManager().addon("testfrontend");
        FCITX_ASSERT(testfrontend);
        auto *module = instance.addonManager().addon("voiceja", true);
        FCITX_ASSERT(module) << "voiceja addon did not load";

        RawConfig raw;
        raw.setValueByPath("SocketPath", socketPath);
        raw.setValueByPath("TapThresholdMs", "250");
        raw.setValueByPath("Language", envOr("VOICE_JA_TEST_LANG", "auto"));
        module->setConfig(raw);

        auto group = instance.inputMethodManager().currentGroup();
        group.inputMethodList().clear();
        group.inputMethodList().emplace_back("keyboard-us");
        group.setDefaultInputMethod("keyboard-us");
        instance.inputMethodManager().setGroup(std::move(group));

        uuid = testfrontend->call<ITestFrontend::createInputContext>("testapp");
        ic = instance.inputContextManager().findByUUID(uuid);
        FCITX_ASSERT(ic);
        ic->focusIn();
        FCITX_ASSERT(instance.inputMethod(ic) == "keyboard-us") << instance.inputMethod(ic);

        commitWatcher = instance.watchEvent(EventType::InputContextCommitString, EventWatcherPhase::Default,
                                            [&](Event &event) {
                                                auto &commit = static_cast<CommitStringEvent &>(event);
                                                FCITX_INFO() << "observed commit: " << commit.text();
                                                ++commits;
                                                lastCommit = commit.text();
                                            });

        // fcitx5 shows the input method name near the cursor for a second
        // after the group change; let it fade so the aux line is ours.
        after(kUsec * 3 / 2, [&]() { holdPhase(); });
    });

    instance.exec();
    if (player > 0) {
        waitpid(player, nullptr, 0);
    }
    FCITX_INFO() << "testvoiceja passed";
    return 0;
}
