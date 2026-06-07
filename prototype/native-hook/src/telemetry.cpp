#include "eos_probe/telemetry.h"

#include <windows.h>

#include <chrono>
#include <fstream>
#include <iomanip>
#include <mutex>
#include <sstream>
#include <string>

namespace eos_probe {
namespace {

std::mutex log_mutex;
std::ofstream log_stream;

std::wstring DefaultLogPath() {
    wchar_t user_profile[MAX_PATH] = {};
    const DWORD size =
        GetEnvironmentVariableW(L"USERPROFILE", user_profile, MAX_PATH);
    if (size > 0 && size < MAX_PATH) {
        return std::wstring(user_profile) +
               L"\\Documents\\My Games\\Binding of Isaac Repentance+"
               L"\\online_logs\\eos_probe.jsonl";
    }

    wchar_t temp_path[MAX_PATH] = {};
    const DWORD temp_size = GetTempPathW(MAX_PATH, temp_path);
    if (temp_size > 0 && temp_size < MAX_PATH) {
        return std::wstring(temp_path) + L"isaac_eos_probe.jsonl";
    }

    return L"isaac_eos_probe.jsonl";
}

void WriteLine(const std::string& line) {
    std::lock_guard<std::mutex> guard(log_mutex);
    if (!log_stream.is_open()) {
        return;
    }
    log_stream << line << '\n';
    log_stream.flush();
}

std::string JsonString(const char* value) {
    std::ostringstream out;
    out << '"';
    if (value != nullptr) {
        for (const unsigned char ch : std::string(value)) {
            switch (ch) {
                case '\\':
                    out << "\\\\";
                    break;
                case '"':
                    out << "\\\"";
                    break;
                case '\b':
                    out << "\\b";
                    break;
                case '\f':
                    out << "\\f";
                    break;
                case '\n':
                    out << "\\n";
                    break;
                case '\r':
                    out << "\\r";
                    break;
                case '\t':
                    out << "\\t";
                    break;
                default:
                    if (ch < 0x20) {
                        out << "\\u" << std::hex << std::setw(4)
                            << std::setfill('0') << static_cast<int>(ch)
                            << std::dec;
                    } else {
                        out << static_cast<char>(ch);
                    }
                    break;
            }
        }
    }
    out << '"';
    return out.str();
}

std::string PointerToJson(const void* value) {
    std::ostringstream out;
    out << "\"0x" << std::hex << reinterpret_cast<std::uintptr_t>(value) << "\"";
    return out.str();
}

}  // namespace

void OpenTelemetry() {
    std::lock_guard<std::mutex> guard(log_mutex);
    if (log_stream.is_open()) {
        return;
    }
    log_stream.open(DefaultLogPath(), std::ios::out | std::ios::app);
}

void CloseTelemetry() {
    std::lock_guard<std::mutex> guard(log_mutex);
    if (log_stream.is_open()) {
        log_stream.flush();
        log_stream.close();
    }
}

std::int64_t NowMicros() {
    const auto now = std::chrono::steady_clock::now().time_since_epoch();
    return std::chrono::duration_cast<std::chrono::microseconds>(now).count();
}

int ReadEnvInt(const wchar_t* name, int fallback) {
    wchar_t value[64] = {};
    const DWORD size = GetEnvironmentVariableW(name, value, 64);
    if (size == 0 || size >= 64) {
        return fallback;
    }
    return _wtoi(value);
}

void LogInstallEvent(const char* status) {
    WriteLine("{\"ts_us\":" + std::to_string(NowMicros()) +
              ",\"event\":\"install\",\"status\":\"" + status + "\"}");
}

void LogPatchEvent(const char* symbol, const char* status) {
    WriteLine("{\"ts_us\":" + std::to_string(NowMicros()) +
              ",\"event\":\"patch\",\"symbol\":" + JsonString(symbol) +
              ",\"status\":" + JsonString(status) + "}");
}

void LogCallEvent(const char* event, const char* api) {
    WriteLine("{\"ts_us\":" + std::to_string(NowMicros()) +
              ",\"event\":" + JsonString(event) +
              ",\"api\":" + JsonString(api) + "}");
}

void LogPointerResultEvent(const char* event, const char* api, const void* result) {
    WriteLine("{\"ts_us\":" + std::to_string(NowMicros()) +
              ",\"event\":" + JsonString(event) +
              ",\"api\":" + JsonString(api) +
              ",\"result\":" + PointerToJson(result) + "}");
}

void LogSteamInterfaceEvent(const char* version, const void* result) {
    WriteLine("{\"ts_us\":" + std::to_string(NowMicros()) +
              ",\"event\":\"steam_interface\",\"version\":" +
              JsonString(version) + ",\"result\":" + PointerToJson(result) + "}");
}

void LogSteamP2PSendEvent(std::int64_t ts_us,
                          bool result,
                          std::uint64_t peer,
                          std::uint32_t bytes,
                          int send_type,
                          int channel) {
    WriteLine("{\"ts_us\":" + std::to_string(ts_us) +
              ",\"event\":\"steam_send\",\"result\":" +
              std::to_string(result ? 1 : 0) +
              ",\"peer\":\"" + std::to_string(peer) +
              "\",\"bytes\":" + std::to_string(bytes) +
              ",\"send_type\":" + std::to_string(send_type) +
              ",\"channel\":" + std::to_string(channel) + "}");
}

void LogSteamP2PAvailableEvent(std::int64_t ts_us,
                               bool result,
                               std::uint32_t bytes,
                               int channel) {
    WriteLine("{\"ts_us\":" + std::to_string(ts_us) +
              ",\"event\":\"steam_available\",\"result\":" +
              std::to_string(result ? 1 : 0) +
              ",\"bytes\":" + std::to_string(bytes) +
              ",\"channel\":" + std::to_string(channel) + "}");
}

void LogSteamP2PReadEvent(std::int64_t ts_us,
                          bool result,
                          std::uint64_t peer,
                          std::uint32_t bytes,
                          std::uint32_t max_bytes,
                          int channel) {
    WriteLine("{\"ts_us\":" + std::to_string(ts_us) +
              ",\"event\":\"steam_recv\",\"result\":" +
              std::to_string(result ? 1 : 0) +
              ",\"peer\":\"" + std::to_string(peer) +
              "\",\"bytes\":" + std::to_string(bytes) +
              ",\"max_bytes\":" + std::to_string(max_bytes) +
              ",\"channel\":" + std::to_string(channel) + "}");
}

void LogSteamP2PSessionStateEvent(std::int64_t ts_us,
                                  bool result,
                                  std::uint64_t peer,
                                  std::uint8_t active,
                                  std::uint8_t connecting,
                                  std::uint8_t error,
                                  std::uint8_t relay,
                                  std::int32_t queued_bytes,
                                  std::int32_t queued_packets) {
    WriteLine("{\"ts_us\":" + std::to_string(ts_us) +
              ",\"event\":\"steam_session_state\",\"result\":" +
              std::to_string(result ? 1 : 0) +
              ",\"peer\":\"" + std::to_string(peer) +
              "\",\"active\":" + std::to_string(active) +
              ",\"connecting\":" + std::to_string(connecting) +
              ",\"error\":" + std::to_string(error) +
              ",\"relay\":" + std::to_string(relay) +
              ",\"queued_bytes\":" + std::to_string(queued_bytes) +
              ",\"queued_packets\":" + std::to_string(queued_packets) + "}");
}

void LogSendEvent(std::int64_t ts_us,
                  std::int32_t result,
                  int channel,
                  std::uint32_t bytes,
                  std::int32_t reliability,
                  std::int32_t delayed_delivery,
                  const char* socket_name) {
    WriteLine("{\"ts_us\":" + std::to_string(ts_us) +
              ",\"event\":\"send\",\"result\":" + std::to_string(result) +
              ",\"channel\":" + std::to_string(channel) +
              ",\"bytes\":" + std::to_string(bytes) +
              ",\"reliability\":" + std::to_string(reliability) +
              ",\"delayed_delivery\":" +
              std::to_string(delayed_delivery) +
              ",\"socket\":" + JsonString(socket_name) + "}");
}

void LogReceiveEvent(std::int64_t ts_us,
                     std::int32_t result,
                     int channel,
                     std::uint32_t bytes,
                     std::uint32_t max_bytes,
                     const char* socket_name) {
    WriteLine("{\"ts_us\":" + std::to_string(ts_us) +
              ",\"event\":\"recv\",\"result\":" + std::to_string(result) +
              ",\"channel\":" + std::to_string(channel) +
              ",\"bytes\":" + std::to_string(bytes) +
              ",\"max_bytes\":" + std::to_string(max_bytes) +
              ",\"socket\":" + JsonString(socket_name) + "}");
}

void LogNextSizeEvent(std::int64_t ts_us,
                      std::int32_t result,
                      std::uint32_t bytes) {
    WriteLine("{\"ts_us\":" + std::to_string(ts_us) +
              ",\"event\":\"next_size\",\"result\":" +
              std::to_string(result) + ",\"bytes\":" +
              std::to_string(bytes) + "}");
}

}  // namespace eos_probe
