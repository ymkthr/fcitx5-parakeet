/*
 * SPDX-License-Identifier: MIT
 *
 * Non-blocking client for the parakeetd Unix-socket line protocol, driven by
 * the fcitx5 event loop so the input method never blocks on transcription.
 */
#ifndef FCITX5_PARAKEET_CLIENT_H
#define FCITX5_PARAKEET_CLIENT_H

#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <string_view>
#include <unordered_map>

#include <fcitx-utils/event.h>
#include <fcitx-utils/unixfd.h>

namespace fcitx {

class ParakeetClient {
public:
    /// Called exactly once per request: ok=false carries the ERR message or a
    /// transport failure description.
    using Reply = std::function<void(bool ok, std::string payload)>;

    ParakeetClient(EventLoop &loop, std::string socketPath);
    ~ParakeetClient();

    void setSocketPath(std::string socketPath);
    const std::string &socketPath() const { return socketPath_; }
    bool connected() const { return fd_.isValid(); }

    /// Sends "<id> COMMAND [arg]". Returns false (without invoking reply) when
    /// the daemon socket cannot be reached. The reply fails with "timeout"
    /// if nothing arrives within timeoutUsec.
    bool request(std::string_view command, std::string_view arg, Reply reply, uint64_t timeoutUsec);

    /// Drops the connection; pending replies fail with the given reason.
    void disconnect(const std::string &reason);

private:
    struct Pending {
        Reply reply;
        std::unique_ptr<EventSourceTime> timer;
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
};

/// $XDG_RUNTIME_DIR/parakeetd.sock (falls back to /run/user/<uid>).
std::string defaultParakeetSocketPath();

} // namespace fcitx

#endif // FCITX5_PARAKEET_CLIENT_H
