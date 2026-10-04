/*
 * SPDX-License-Identifier: MIT
 *
 * Non-blocking client for the voice-jad Unix-socket line protocol, driven by
 * the fcitx5 event loop so the input method never blocks on transcription.
 */
#ifndef FCITX5_VOICE_JA_CLIENT_H
#define FCITX5_VOICE_JA_CLIENT_H

#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <string_view>
#include <unordered_map>

#include <fcitx-utils/event.h>
#include <fcitx-utils/unixfd.h>

namespace fcitx {

class VoiceJaClient {
public:
    /// Called once per request: ok=false carries the ERR message or a
    /// transport failure description.
    using Reply = std::function<void(bool ok, std::string payload)>;
    /// Receives each `<id> <EVENT> [payload]` line (PARTIAL, COMMIT, ENDED).
    using Event = std::function<void(const std::string &event, std::string payload)>;

    VoiceJaClient(EventLoop &loop, std::string socketPath);
    ~VoiceJaClient();

    void setSocketPath(std::string socketPath);
    const std::string &socketPath() const { return socketPath_; }
    bool connected() const { return fd_.isValid(); }

    /// Sends "<id> COMMAND [arg]" and returns the id, or 0 (without invoking
    /// reply) when the daemon socket cannot be reached. If nothing arrives
    /// within timeoutUsec the reply fails with "timeout", unless onTimeout is
    /// set: then onTimeout runs and the reply still comes whenever the daemon
    /// sends it. onEvent, if set, receives this id's events, which outlive the
    /// reply, until unsubscribe(id) or disconnect().
    uint64_t request(std::string_view command, std::string_view arg, Reply reply, uint64_t timeoutUsec,
                     Event onEvent = {}, std::function<void()> onTimeout = {});

    void unsubscribe(uint64_t id) { events_.erase(id); }

    /// Drops the connection; pending replies fail with the given reason.
    void disconnect(const std::string &reason);

private:
    struct Pending {
        Reply reply;
        std::unique_ptr<EventSourceTime> timer;
        std::function<void()> onTimeout;
    };

    bool ensureConnected();
    bool writeAll(std::string_view data);
    void onReadable();
    void handleLine(std::string_view line);
    void settle(uint64_t id, bool ok, std::string payload);

    EventLoop &loop_;
    std::string socketPath_;
    UnixFD fd_;
    std::unique_ptr<EventSourceIO> ioEvent_;
    std::string inbuf_;
    uint64_t nextId_ = 1;
    std::unordered_map<uint64_t, Pending> pending_;
    std::unordered_map<uint64_t, Event> events_;
};

/// $XDG_RUNTIME_DIR/voice-jad.sock (falls back to /run/user/<uid>).
std::string defaultVoiceJaSocketPath();

} // namespace fcitx

#endif // FCITX5_VOICE_JA_CLIENT_H
