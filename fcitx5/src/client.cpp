/*
 * SPDX-License-Identifier: MIT
 */
#include "client.h"

#include <cerrno>
#include <charconv>
#include <cstring>
#include <utility>

#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <fcitx-utils/log.h>

namespace fcitx {

FCITX_DEFINE_LOG_CATEGORY(parakeet_client, "parakeet-client");
#define PK_DEBUG() FCITX_LOGC(parakeet_client, Debug)
#define PK_WARN() FCITX_LOGC(parakeet_client, Warn)

std::string defaultParakeetSocketPath() {
    if (const char *runtime = std::getenv("XDG_RUNTIME_DIR"); runtime && *runtime) {
        return std::string(runtime) + "/parakeetd.sock";
    }
    return "/run/user/" + std::to_string(getuid()) + "/parakeetd.sock";
}

ParakeetClient::ParakeetClient(EventLoop &loop, std::string socketPath)
    : loop_(loop), socketPath_(std::move(socketPath)) {}

ParakeetClient::~ParakeetClient() { disconnect("client destroyed"); }

void ParakeetClient::setSocketPath(std::string socketPath) {
    if (socketPath == socketPath_) {
        return;
    }
    socketPath_ = std::move(socketPath);
    disconnect("socket path changed");
}

bool ParakeetClient::ensureConnected() {
    if (fd_.isValid()) {
        return true;
    }
    sockaddr_un addr{};
    addr.sun_family = AF_UNIX;
    if (socketPath_.size() >= sizeof(addr.sun_path)) {
        PK_WARN() << "socket path too long: " << socketPath_;
        return false;
    }
    std::memcpy(addr.sun_path, socketPath_.c_str(), socketPath_.size() + 1);

    UnixFD fd = UnixFD::own(::socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0));
    if (!fd.isValid()) {
        PK_WARN() << "socket(): " << std::strerror(errno);
        return false;
    }
    // AF_UNIX connect completes synchronously unless the listen backlog is
    // full; systemd socket activation accepts immediately and buffers our
    // request until parakeetd is up.
    if (::connect(fd.fd(), reinterpret_cast<const sockaddr *>(&addr), sizeof(addr)) < 0 &&
        errno != EINPROGRESS && errno != EAGAIN) {
        PK_WARN() << "connect(" << socketPath_ << "): " << std::strerror(errno);
        return false;
    }
    fd_ = std::move(fd);
    inbuf_.clear();
    ioEvent_ = loop_.addIOEvent(fd_.fd(), IOEventFlags{IOEventFlag::In, IOEventFlag::Err, IOEventFlag::Hup},
                                [this](EventSourceIO *, int, IOEventFlags flags) {
                                    if (flags.test(IOEventFlag::In)) {
                                        onReadable();
                                    } else {
                                        disconnect("daemon closed the connection");
                                    }
                                    return true;
                                });
    PK_DEBUG() << "connected to " << socketPath_;
    return true;
}

void ParakeetClient::disconnect(const std::string &reason) {
    ioEvent_.reset();
    fd_.reset();
    inbuf_.clear();
    if (pending_.empty()) {
        return;
    }
    auto pending = std::move(pending_);
    pending_.clear();
    for (auto &[id, entry] : pending) {
        entry.timer.reset();
        entry.reply(false, reason);
    }
}

bool ParakeetClient::writeAll(std::string_view data) {
    while (!data.empty()) {
        const ssize_t n = ::send(fd_.fd(), data.data(), data.size(), MSG_NOSIGNAL);
        if (n < 0) {
            if (errno == EINTR) {
                continue;
            }
            // Requests are a few dozen bytes; a full socket buffer means the
            // daemon is wedged, so treat it like a broken pipe.
            PK_WARN() << "send(): " << std::strerror(errno);
            return false;
        }
        data.remove_prefix(static_cast<size_t>(n));
    }
    return true;
}

bool ParakeetClient::request(std::string_view command, std::string_view arg, Reply reply, uint64_t timeoutUsec) {
    if (!ensureConnected()) {
        return false;
    }
    const uint64_t id = nextId_++;
    std::string line = std::to_string(id);
    line += ' ';
    line += command;
    if (!arg.empty()) {
        line += ' ';
        line += arg;
    }
    line += '\n';
    if (!writeAll(line)) {
        disconnect("write failed");
        return false;
    }
    auto timer = loop_.addTimeEvent(CLOCK_MONOTONIC, now(CLOCK_MONOTONIC) + timeoutUsec, 0,
                                    [this, id](EventSourceTime *source, uint64_t) {
                                        source->setEnabled(false);
                                        settle(id, false, "timeout");
                                        return true;
                                    });
    pending_.emplace(id, Pending{std::move(reply), std::move(timer)});
    return true;
}

void ParakeetClient::settle(uint64_t id, bool ok, std::string payload) {
    PK_DEBUG() << "settle request " << id << " ok=" << ok << " pending=" << pending_.size();
    auto it = pending_.find(id);
    if (it == pending_.end()) {
        PK_DEBUG() << "reply for unknown request " << id;
        return;
    }
    // Move the entry out first: the reply may issue new requests, and the
    // timer must outlive its own callback frame.
    Pending entry = std::move(it->second);
    pending_.erase(it);
    entry.reply(ok, std::move(payload));
}

void ParakeetClient::onReadable() {
    char buf[4096];
    for (;;) {
        const ssize_t n = ::recv(fd_.fd(), buf, sizeof(buf), 0);
        if (n > 0) {
            inbuf_.append(buf, static_cast<size_t>(n));
            continue;
        }
        if (n < 0 && errno == EINTR) {
            continue;
        }
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
            break;
        }
        disconnect(n == 0 ? "daemon closed the connection" : std::strerror(errno));
        return;
    }
    size_t start = 0;
    for (;;) {
        const size_t nl = inbuf_.find('\n', start);
        if (nl == std::string::npos) {
            break;
        }
        handleLine(std::string_view(inbuf_).substr(start, nl - start));
        start = nl + 1;
    }
    inbuf_.erase(0, start);
}

void ParakeetClient::handleLine(std::string_view line) {
    const size_t idEnd = line.find(' ');
    if (idEnd == std::string_view::npos) {
        PK_WARN() << "malformed reply: " << std::string(line);
        return;
    }
    uint64_t id = 0;
    const auto idText = line.substr(0, idEnd);
    if (std::from_chars(idText.data(), idText.data() + idText.size(), id).ec != std::errc{}) {
        PK_WARN() << "malformed reply id: " << std::string(line);
        return;
    }
    std::string_view rest = line.substr(idEnd + 1);
    const size_t statusEnd = rest.find(' ');
    const std::string_view status = rest.substr(0, statusEnd);
    const std::string_view payload = statusEnd == std::string_view::npos ? std::string_view{} : rest.substr(statusEnd + 1);

    settle(id, status == "OK", std::string(payload));
}

} // namespace fcitx
