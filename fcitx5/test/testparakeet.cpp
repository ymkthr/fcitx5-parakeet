/*
 * SPDX-License-Identifier: MIT
 *
 * Drives the parakeet engine through fcitx5's test frontend against a live
 * parakeetd. Verifies:
 *   1. a quick tap of the trigger types the key itself,
 *   2. unrelated keys are not swallowed,
 *   3. holding the trigger dictates: the transcript is committed on release.
 *
 * Environment:
 *   PARAKEET_TEST_SOCKET  parakeetd socket (unset -> test skipped, exit 77)
 *   PARAKEET_TEST_WAV     wav played into PARAKEET_TEST_SINK while holding
 *   PARAKEET_TEST_SINK    PipeWire sink whose monitor parakeetd captures
 *   PARAKEET_TEST_EXPECT  exact transcript to require (optional; any commit otherwise)
 *   PARAKEET_TEST_LANG    input method language, ja or en (default en)
 *   PARAKEET_TEST_HOLD_SEC seconds to hold the trigger while the wav plays (default 8)
 */
#include <chrono>
#include <cstdlib>
#include <string>

#include <spawn.h>
#include <sys/wait.h>

#include <fcitx-config/rawconfig.h>
#include <fcitx-utils/eventdispatcher.h>
#include <fcitx-utils/event.h>
#include <fcitx-utils/log.h>
#include <fcitx-utils/testing.h>
#include <fcitx/addonmanager.h>
#include <fcitx/event.h>
#include <fcitx/inputcontextmanager.h>
#include <fcitx/inputpanel.h>
#include <fcitx/inputmethodmanager.h>
#include <fcitx/instance.h>
#include <testfrontend_public.h>

using namespace fcitx;

extern char **environ;

namespace {

constexpr uint64_t kUsec = 1000 * 1000;

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
    const char *socketPath = std::getenv("PARAKEET_TEST_SOCKET");
    if (!socketPath) {
        FCITX_INFO() << "PARAKEET_TEST_SOCKET not set; skipping";
        return 77;
    }
    const std::string wav = envOr("PARAKEET_TEST_WAV", "");
    const std::string sink = envOr("PARAKEET_TEST_SINK", "");
    // Exact-match check is optional: ASR output for some clips is not stable
    // across capture offsets, but a played wav must always produce a commit.
    const std::string expect = envOr("PARAKEET_TEST_EXPECT", "");
    const bool expectCommit = !wav.empty() && !sink.empty();
    const std::string im = "parakeet-" + envOr("PARAKEET_TEST_LANG", "en");

    setupTestingEnvironment(TESTING_BINARY_DIR, {"src"}, {"test"});
    char arg0[] = "testparakeet";
    char arg1[] = "--disable=all";
    char arg2[] = "--enable=testim,testfrontend,keyboard,parakeet";
    // PARAKEET_TEST_VERBOSE=1 turns on the addon's own log categories.
    char arg3[] = "--verbose=parakeet=5,parakeet-client=5";
    char *argv[] = {arg0, arg1, arg2, arg3};
    const int argc = std::getenv("PARAKEET_TEST_VERBOSE") ? 4 : 3;
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
    std::unique_ptr<EventSourceTime> playTimer;
    std::unique_ptr<EventSourceTime> releaseTimer;
    std::unique_ptr<EventSourceTime> deadline;
    pid_t player = 0;

    dispatcher.schedule([&]() {
        testfrontend = instance.addonManager().addon("testfrontend");
        FCITX_ASSERT(testfrontend);
        auto *engine = instance.addonManager().addon("parakeet", true);
        FCITX_ASSERT(engine) << "parakeet addon did not load";

        RawConfig raw;
        raw.setValueByPath("SocketPath", socketPath);
        raw.setValueByPath("TapThresholdMs", "250");
        engine->setConfig(raw);

        auto group = instance.inputMethodManager().currentGroup();
        group.inputMethodList().clear();
        group.inputMethodList().emplace_back("keyboard-us");
        group.inputMethodList().emplace_back(im);
        group.setDefaultInputMethod(im);
        instance.inputMethodManager().setGroup(std::move(group));

        uuid = testfrontend->call<ITestFrontend::createInputContext>("testapp");
        ic = instance.inputContextManager().findByUUID(uuid);
        FCITX_ASSERT(ic);
        ic->focusIn();
        instance.setCurrentInputMethod(ic, im, false);
        FCITX_ASSERT(instance.inputMethod(ic) == im) << instance.inputMethod(ic);

        commitWatcher = instance.watchEvent(EventType::InputContextCommitString, EventWatcherPhase::Default,
                                            [&](Event &event) {
                                                auto &commit = static_cast<CommitStringEvent &>(event);
                                                FCITX_INFO() << "observed commit: " << commit.text();
                                                ++commits;
                                                lastCommit = commit.text();
                                                if (expectCommit && commits == 2) {
                                                    FCITX_ASSERT(!lastCommit.empty());
                                                    if (!expect.empty()) {
                                                        FCITX_ASSERT(lastCommit == expect) << lastCommit;
                                                    }
                                                    instance.exit();
                                                }
                                            });

        testfrontend->call<ITestFrontend::keyEvent>(uuid, Key("space"), false);
        testfrontend->call<ITestFrontend::keyEvent>(uuid, Key("space"), true);
        FCITX_ASSERT(commits == 1 && lastCommit == " ") << "tap did not type a space";

        FCITX_ASSERT(!testfrontend->call<ITestFrontend::sendKeyEvent>(uuid, Key("a"), false));
        FCITX_ASSERT(!testfrontend->call<ITestFrontend::sendKeyEvent>(uuid, Key("a"), true));
        FCITX_ASSERT(!testfrontend->call<ITestFrontend::sendKeyEvent>(uuid, Key("Control+c"), false));
        FCITX_ASSERT(commits == 1);

        // 3. Hold to dictate. Auto-repeat presses must be swallowed, and the
        //    toolkit-style reset() apps issue on cursor moves must not cancel.
        testfrontend->call<ITestFrontend::keyEvent>(uuid, Key("space"), false);
        FCITX_ASSERT(testfrontend->call<ITestFrontend::sendKeyEvent>(uuid, Key("space"), false));
        ic->reset();
        FCITX_ASSERT(commits == 1);
        // Nothing is shown until parakeetd confirms samples are flowing.
        FCITX_ASSERT(ic->inputPanel().auxUp().toString().empty()) << ic->inputPanel().auxUp().toString();

        uint64_t holdUsec = 1 * kUsec;
        if (!wav.empty() && !sink.empty()) {
            // Let the capture stream become live before playback starts, as a
            // person naturally pauses between pressing the key and speaking.
            playTimer = instance.eventLoop().addTimeEvent(
                CLOCK_MONOTONIC, now(CLOCK_MONOTONIC) + kUsec / 2, 0, [&](EventSourceTime *source, uint64_t) {
                    source->setEnabled(false);
                    player = play(sink, wav);
                    return true;
                });
            holdUsec = static_cast<uint64_t>(std::stod(envOr("PARAKEET_TEST_HOLD_SEC", "8")) * kUsec);
        }
        releaseTimer = instance.eventLoop().addTimeEvent(
            CLOCK_MONOTONIC, now(CLOCK_MONOTONIC) + holdUsec, 0, [&](EventSourceTime *source, uint64_t) {
                source->setEnabled(false);
                FCITX_ASSERT(instance.inputMethodIcon(ic) == "fcitx-parakeet-recording")
                    << instance.inputMethodIcon(ic);
                testfrontend->call<ITestFrontend::keyEvent>(uuid, Key("space"), true);
                return true;
            });
        deadline = instance.eventLoop().addTimeEvent(
            CLOCK_MONOTONIC, now(CLOCK_MONOTONIC) + holdUsec + 15 * kUsec, 0, [&](EventSourceTime *, uint64_t) {
                if (expectCommit) {
                    FCITX_ASSERT(commits == 2) << "transcript was never committed";
                } else {
                    FCITX_ASSERT(commits == 1) << "silence must not commit anything";
                }
                instance.exit();
                return true;
            });
    });

    instance.exec();
    if (player > 0) {
        waitpid(player, nullptr, 0);
    }
    FCITX_INFO() << "testparakeet passed";
    return 0;
}
