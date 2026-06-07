#pragma once

#include <cstdint>
#include <string>

namespace eos_probe {

void OpenTelemetry();
void CloseTelemetry();
void LogInstallEvent(const char* status);
void LogPatchEvent(const char* symbol, const char* status);
void LogCallEvent(const char* event, const char* api);
void LogPointerResultEvent(const char* event, const char* api, const void* result);
void LogSteamInterfaceEvent(const char* version, const void* result);
void LogSteamP2PSendEvent(std::int64_t ts_us,
                          bool result,
                          std::uint64_t peer,
                          std::uint32_t bytes,
                          int send_type,
                          int channel);
void LogSteamP2PAvailableEvent(std::int64_t ts_us,
                               bool result,
                               std::uint32_t bytes,
                               int channel);
void LogSteamP2PReadEvent(std::int64_t ts_us,
                          bool result,
                          std::uint64_t peer,
                          std::uint32_t bytes,
                          std::uint32_t max_bytes,
                          int channel);
void LogSteamP2PSessionStateEvent(std::int64_t ts_us,
                                  bool result,
                                  std::uint64_t peer,
                                  std::uint8_t active,
                                  std::uint8_t connecting,
                                  std::uint8_t error,
                                  std::uint8_t relay,
                                  std::int32_t queued_bytes,
                                  std::int32_t queued_packets);
void LogSendEvent(std::int64_t ts_us,
                  std::int32_t result,
                  int channel,
                  std::uint32_t bytes,
                  std::int32_t reliability,
                  std::int32_t delayed_delivery,
                  const char* socket_name);
void LogReceiveEvent(std::int64_t ts_us,
                     std::int32_t result,
                     int channel,
                     std::uint32_t bytes,
                     std::uint32_t max_bytes,
                     const char* socket_name);
void LogNextSizeEvent(std::int64_t ts_us,
                      std::int32_t result,
                      std::uint32_t bytes);
std::int64_t NowMicros();
int ReadEnvInt(const wchar_t* name, int fallback);

}  // namespace eos_probe
