#include "eos_probe/bridge.h"

#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>

#include <algorithm>
#include <atomic>
#include <cstddef>
#include <cwctype>
#include <cstring>
#include <deque>
#include <fstream>
#include <mutex>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

#include "eos_probe/telemetry.h"

namespace eos_probe {
namespace {

constexpr std::size_t kMaxPayloadSize = 64 * 1024;
constexpr std::size_t kMaxQueuedPackets = 4096;
constexpr char kLocalMagic[4] = {'I', 'B', 'R', '1'};
constexpr std::uint8_t kVersion = 1;
constexpr std::uint8_t kTypeOutgoing = 1;
constexpr std::uint8_t kTypeIncoming = 2;

#pragma pack(push, 1)
struct LocalPacketHeader {
    char magic[4];
    std::uint8_t version;
    std::uint8_t type;
    std::uint16_t header_size;
    std::uint64_t peer;
    std::uint32_t sequence;
    std::int32_t channel;
    std::int32_t send_type;
    std::uint32_t payload_size;
};
#pragma pack(pop)

struct QueuedPacket {
    std::uint64_t peer = 0;
    std::uint32_t sequence = 0;
    int channel = 0;
    int send_type = 0;
    std::vector<std::uint8_t> payload;
};

std::atomic<bool> initialized{false};
std::atomic<bool> running{false};
BridgeMode mode = BridgeMode::Off;
bool fallback_to_steam = true;
SOCKET send_socket = INVALID_SOCKET;
SOCKET receive_socket = INVALID_SOCKET;
sockaddr_in sidecar_address = {};
std::thread receive_thread;
std::mutex queue_mutex;
std::deque<QueuedPacket> queue;
std::atomic<std::uint32_t> next_sequence{1};

struct BridgeConfig {
    bool loaded = false;
    std::wstring mode;
    std::wstring fallback_to_steam;
    std::wstring sidecar;
    std::wstring bind;
};

std::wstring ReadEnvString(const wchar_t* name) {
    wchar_t buffer[512] = {};
    const DWORD size = GetEnvironmentVariableW(name, buffer, 512);
    if (size == 0 || size >= 512) {
        return L"";
    }
    return buffer;
}

std::wstring DefaultConfigPath() {
    wchar_t user_profile[MAX_PATH] = {};
    const DWORD size =
        GetEnvironmentVariableW(L"USERPROFILE", user_profile, MAX_PATH);
    if (size > 0 && size < MAX_PATH) {
        return std::wstring(user_profile) +
               L"\\Documents\\My Games\\Binding of Isaac Repentance+"
               L"\\online_logs\\isaac_bridge_config.txt";
    }

    wchar_t temp_path[MAX_PATH] = {};
    const DWORD temp_size = GetTempPathW(MAX_PATH, temp_path);
    if (temp_size > 0 && temp_size < MAX_PATH) {
        return std::wstring(temp_path) + L"isaac_bridge_config.txt";
    }

    return L"isaac_bridge_config.txt";
}

std::wstring Trim(std::wstring value) {
    const auto first = std::find_if_not(value.begin(), value.end(), [](wchar_t ch) {
        return std::iswspace(ch) != 0;
    });
    const auto last = std::find_if_not(value.rbegin(), value.rend(), [](wchar_t ch) {
        return std::iswspace(ch) != 0;
    }).base();
    if (first >= last) {
        return L"";
    }
    return std::wstring(first, last);
}

std::wstring LowerAscii(std::wstring value) {
    for (auto& ch : value) {
        if (ch >= L'A' && ch <= L'Z') {
            ch = static_cast<wchar_t>(ch - L'A' + L'a');
        }
    }
    return value;
}

bool ParseBoolSetting(const std::wstring& value, bool fallback) {
    const std::wstring normalized = LowerAscii(Trim(value));
    if (normalized == L"1" || normalized == L"true" ||
        normalized == L"yes" || normalized == L"on") {
        return true;
    }
    if (normalized == L"0" || normalized == L"false" ||
        normalized == L"no" || normalized == L"off") {
        return false;
    }
    return fallback;
}

BridgeConfig ReadBridgeConfigFile() {
    BridgeConfig config;
    std::wifstream file(DefaultConfigPath());
    if (!file.is_open()) {
        return config;
    }

    config.loaded = true;
    std::wstring line;
    while (std::getline(file, line)) {
        line = Trim(line);
        if (line.empty() || line[0] == L'#') {
            continue;
        }

        const auto separator = line.find(L'=');
        if (separator == std::wstring::npos) {
            continue;
        }

        const std::wstring key = LowerAscii(Trim(line.substr(0, separator)));
        const std::wstring value = Trim(line.substr(separator + 1));
        if (key == L"mode") {
            config.mode = value;
        } else if (key == L"fallback_to_steam") {
            config.fallback_to_steam = value;
        } else if (key == L"sidecar") {
            config.sidecar = value;
        } else if (key == L"bind") {
            config.bind = value;
        }
    }

    return config;
}

std::wstring FirstNonEmpty(const std::wstring& first,
                           const std::wstring& second) {
    return first.empty() ? second : first;
}

std::string NarrowAscii(const std::wstring& value) {
    std::string output;
    output.reserve(value.size());
    for (const wchar_t ch : value) {
        output.push_back(ch <= 0x7f ? static_cast<char>(ch) : '?');
    }
    return output;
}

BridgeMode ParseMode(const std::wstring& value) {
    if (value == L"mirror" || value == L"MIRROR") {
        return BridgeMode::Mirror;
    }
    if (value == L"replace" || value == L"REPLACE") {
        return BridgeMode::Replace;
    }
    return BridgeMode::Off;
}

bool ParseEndpoint(const std::wstring& value,
                   const char* fallback_host,
                   unsigned short fallback_port,
                   sockaddr_in* out_address) {
    std::string endpoint = NarrowAscii(value);
    std::string host = fallback_host;
    unsigned short port = fallback_port;

    if (!endpoint.empty()) {
        const auto separator = endpoint.rfind(':');
        if (separator != std::string::npos) {
            host = endpoint.substr(0, separator);
            const int parsed_port = std::atoi(endpoint.substr(separator + 1).c_str());
            if (parsed_port > 0 && parsed_port <= 65535) {
                port = static_cast<unsigned short>(parsed_port);
            }
        }
    }

    std::memset(out_address, 0, sizeof(*out_address));
    out_address->sin_family = AF_INET;
    out_address->sin_port = htons(port);
    if (inet_pton(AF_INET, host.c_str(), &out_address->sin_addr) != 1) {
        return false;
    }
    return true;
}

void PushIncomingPacket(const LocalPacketHeader& header,
                        const std::uint8_t* payload) {
    QueuedPacket packet;
    packet.peer = header.peer;
    packet.sequence = header.sequence;
    packet.channel = header.channel;
    packet.send_type = header.send_type;
    packet.payload.assign(payload, payload + header.payload_size);

    std::lock_guard<std::mutex> guard(queue_mutex);
    if (queue.size() >= kMaxQueuedPackets) {
        queue.pop_front();
        LogCallEvent("bridge_queue_drop_oldest", "bridge");
    }
    queue.push_back(std::move(packet));
}

void ReceiveLoop() {
    std::vector<std::uint8_t> buffer(sizeof(LocalPacketHeader) + kMaxPayloadSize);
    while (running.load()) {
        sockaddr_in from = {};
        int from_length = sizeof(from);
        const int received = recvfrom(receive_socket,
                                      reinterpret_cast<char*>(buffer.data()),
                                      static_cast<int>(buffer.size()),
                                      0,
                                      reinterpret_cast<sockaddr*>(&from),
                                      &from_length);
        if (received == SOCKET_ERROR) {
            const int error = WSAGetLastError();
            if (!running.load() || error == WSAEINTR || error == WSAENOTSOCK) {
                break;
            }
            Sleep(1);
            continue;
        }

        if (received < static_cast<int>(sizeof(LocalPacketHeader))) {
            continue;
        }

        LocalPacketHeader header = {};
        std::memcpy(&header, buffer.data(), sizeof(header));
        if (std::memcmp(header.magic, kLocalMagic, sizeof(header.magic)) != 0 ||
            header.version != kVersion ||
            header.type != kTypeIncoming ||
            header.header_size != sizeof(LocalPacketHeader) ||
            header.payload_size > kMaxPayloadSize ||
            received < static_cast<int>(sizeof(LocalPacketHeader) + header.payload_size)) {
            continue;
        }

        PushIncomingPacket(header, buffer.data() + sizeof(LocalPacketHeader));
    }
}

bool BindReceiveSocket(const sockaddr_in& bind_address) {
    receive_socket = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (receive_socket == INVALID_SOCKET) {
        return false;
    }

    if (bind(receive_socket,
             reinterpret_cast<const sockaddr*>(&bind_address),
             sizeof(bind_address)) == SOCKET_ERROR) {
        closesocket(receive_socket);
        receive_socket = INVALID_SOCKET;
        return false;
    }

    return true;
}

}  // namespace

void InitializeBridgeFromEnvironment() {
    bool expected = false;
    if (!initialized.compare_exchange_strong(expected, true)) {
        return;
    }

    const BridgeConfig file_config = ReadBridgeConfigFile();
    if (file_config.loaded) {
        LogCallEvent("bridge_config_file_loaded", "bridge");
    }

    mode = ParseMode(
        FirstNonEmpty(ReadEnvString(L"ISAAC_BRIDGE_MODE"), file_config.mode));
    if (mode == BridgeMode::Off) {
        LogCallEvent("bridge_disabled", "bridge");
        return;
    }

    fallback_to_steam = ParseBoolSetting(file_config.fallback_to_steam, true);
    if (GetEnvironmentVariableW(L"ISAAC_BRIDGE_NO_STEAM_FALLBACK", nullptr, 0) != 0) {
        fallback_to_steam = false;
    }

    WSADATA data = {};
    if (WSAStartup(MAKEWORD(2, 2), &data) != 0) {
        mode = BridgeMode::Off;
        LogCallEvent("bridge_wsa_start_failed", "bridge");
        return;
    }

    if (!ParseEndpoint(FirstNonEmpty(ReadEnvString(L"ISAAC_BRIDGE_SIDECAR"),
                                     file_config.sidecar),
                       "127.0.0.1",
                       25900,
                       &sidecar_address)) {
        mode = BridgeMode::Off;
        LogCallEvent("bridge_bad_sidecar_endpoint", "bridge");
        WSACleanup();
        return;
    }

    sockaddr_in bind_address = {};
    if (!ParseEndpoint(FirstNonEmpty(ReadEnvString(L"ISAAC_BRIDGE_BIND"),
                                     file_config.bind),
                       "127.0.0.1",
                       25901,
                       &bind_address)) {
        mode = BridgeMode::Off;
        LogCallEvent("bridge_bad_bind_endpoint", "bridge");
        WSACleanup();
        return;
    }

    send_socket = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (send_socket == INVALID_SOCKET || !BindReceiveSocket(bind_address)) {
        mode = BridgeMode::Off;
        LogCallEvent("bridge_socket_failed", "bridge");
        if (send_socket != INVALID_SOCKET) {
            closesocket(send_socket);
            send_socket = INVALID_SOCKET;
        }
        WSACleanup();
        return;
    }

    running = true;
    receive_thread = std::thread(ReceiveLoop);
    LogCallEvent(mode == BridgeMode::Replace ? "bridge_replace_enabled"
                                             : "bridge_mirror_enabled",
                 "bridge");
}

void ShutdownBridge() {
    if (!initialized.load()) {
        return;
    }
    running = false;
    if (receive_socket != INVALID_SOCKET) {
        closesocket(receive_socket);
        receive_socket = INVALID_SOCKET;
    }
    if (receive_thread.joinable()) {
        receive_thread.join();
    }
    if (send_socket != INVALID_SOCKET) {
        closesocket(send_socket);
        send_socket = INVALID_SOCKET;
    }
    WSACleanup();
}

BridgeMode GetBridgeMode() {
    return mode;
}

bool IsBridgeEnabled() {
    return mode != BridgeMode::Off;
}

bool ShouldFallbackToSteam() {
    return fallback_to_steam;
}

bool SendBridgePacket(const BridgePacket& packet) {
    if (!IsBridgeEnabled() || send_socket == INVALID_SOCKET ||
        packet.data == nullptr || packet.size > kMaxPayloadSize) {
        return false;
    }

    LocalPacketHeader header = {};
    std::memcpy(header.magic, kLocalMagic, sizeof(header.magic));
    header.version = kVersion;
    header.type = kTypeOutgoing;
    header.header_size = sizeof(LocalPacketHeader);
    header.peer = packet.peer;
    header.sequence = next_sequence.fetch_add(1);
    header.channel = packet.channel;
    header.send_type = packet.send_type;
    header.payload_size = packet.size;

    std::vector<std::uint8_t> frame(sizeof(header) + packet.size);
    std::memcpy(frame.data(), &header, sizeof(header));
    std::memcpy(frame.data() + sizeof(header), packet.data, packet.size);

    const int sent = sendto(send_socket,
                            reinterpret_cast<const char*>(frame.data()),
                            static_cast<int>(frame.size()),
                            0,
                            reinterpret_cast<const sockaddr*>(&sidecar_address),
                            sizeof(sidecar_address));
    return sent == static_cast<int>(frame.size());
}

bool HasBridgePacket(int channel, std::uint32_t* out_size) {
    if (!IsBridgeEnabled()) {
        return false;
    }

    std::lock_guard<std::mutex> guard(queue_mutex);
    for (const auto& packet : queue) {
        if (packet.channel == channel) {
            if (out_size != nullptr) {
                *out_size = static_cast<std::uint32_t>(packet.payload.size());
            }
            return true;
        }
    }
    return false;
}

bool ReadBridgePacket(int channel,
                      void* destination,
                      std::uint32_t max_size,
                      std::uint32_t* out_size,
                      std::uint64_t* out_peer) {
    if (!IsBridgeEnabled() || destination == nullptr) {
        return false;
    }

    std::lock_guard<std::mutex> guard(queue_mutex);
    const auto found = std::find_if(queue.begin(), queue.end(), [channel](const auto& packet) {
        return packet.channel == channel;
    });
    if (found == queue.end()) {
        return false;
    }

    if (found->payload.size() > max_size) {
        if (out_size != nullptr) {
            *out_size = static_cast<std::uint32_t>(found->payload.size());
        }
        return false;
    }

    if (!found->payload.empty()) {
        std::memcpy(destination, found->payload.data(), found->payload.size());
    }
    if (out_size != nullptr) {
        *out_size = static_cast<std::uint32_t>(found->payload.size());
    }
    if (out_peer != nullptr) {
        *out_peer = found->peer;
    }
    queue.erase(found);
    return true;
}

}  // namespace eos_probe
