#include "eos_probe/eos_hooks.h"

#include <windows.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstring>
#include <thread>

#include "eos_probe/bridge.h"
#include "eos_probe/eos_types.h"
#include "eos_probe/iat_patch.h"
#include "eos_probe/telemetry.h"

namespace eos_probe {
namespace {

constexpr EosResult kEosSuccess = 0;

std::atomic<bool> installed{false};
EosP2PSendPacketFn original_send_packet = nullptr;
EosP2PReceivePacketFn original_receive_packet = nullptr;
EosP2PGetNextReceivedPacketSizeFn original_next_size = nullptr;
EosPlatformTickFn original_platform_tick = nullptr;
EosPlatformCreateFn original_platform_create = nullptr;
EosPlatformGetInterfaceFn original_get_p2p_interface = nullptr;
EosPlatformGetInterfaceFn original_get_lobby_interface = nullptr;
EosLobbyAsync4Fn original_lobby_create = nullptr;
EosLobbyAsync4Fn original_lobby_join = nullptr;
EosLobbyAsync4Fn original_lobby_leave = nullptr;
EosLobbyAsync4Fn original_lobby_search_find = nullptr;
EosLobbyCreateSearchFn original_lobby_create_search = nullptr;
SteamFindOrCreateUserInterfaceFn original_steam_find_user_interface = nullptr;
SteamRunCallbacksFn original_steam_run_callbacks = nullptr;
SteamSendP2PPacketFn original_steam_send_p2p_packet = nullptr;
SteamIsP2PPacketAvailableFn original_steam_is_p2p_packet_available = nullptr;
SteamReadP2PPacketFn original_steam_read_p2p_packet = nullptr;
SteamGetP2PSessionStateFn original_steam_get_p2p_session_state = nullptr;
int delay_send_ms = 0;
int delay_recv_ms = 0;
std::atomic<bool> steam_callbacks_logged{false};
std::atomic<bool> steam_networking_hooked{false};
std::atomic<std::int64_t> last_steam_session_state_log_us{0};
std::atomic<std::int64_t> last_steam_available_false_log_us{0};
std::atomic<std::int64_t> last_steam_callbacks_log_us{0};

void SleepIfConfigured(int millis) {
    if (millis <= 0) {
        return;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(millis));
}

int ChannelOrAny(const std::uint8_t* channel) {
    return channel == nullptr ? -1 : static_cast<int>(*channel);
}

const char* SocketName(const EosP2PSocketId* socket_id) {
    if (socket_id == nullptr || socket_id->socket_name[0] == '\0') {
        return "";
    }
    return socket_id->socket_name;
}

void MaybeLogSteamSessionState(void* self,
                               std::uint64_t remote,
                               std::int64_t now_us);

EosHandle __stdcall HookPlatformCreate(const void* options) {
    EosHandle result = nullptr;
    if (original_platform_create != nullptr) {
        result = original_platform_create(options);
    }
    LogPointerResultEvent("eos_platform_create", "_EOS_Platform_Create@4", result);
    return result;
}

EosHandle __stdcall HookGetP2PInterface(EosHandle handle) {
    EosHandle result = nullptr;
    if (original_get_p2p_interface != nullptr) {
        result = original_get_p2p_interface(handle);
    }
    LogPointerResultEvent(
        "eos_get_p2p_interface", "_EOS_Platform_GetP2PInterface@4", result);
    return result;
}

EosHandle __stdcall HookGetLobbyInterface(EosHandle handle) {
    EosHandle result = nullptr;
    if (original_get_lobby_interface != nullptr) {
        result = original_get_lobby_interface(handle);
    }
    LogPointerResultEvent(
        "eos_get_lobby_interface", "_EOS_Platform_GetLobbyInterface@4", result);
    return result;
}

void __stdcall HookLobbyCreate(EosHandle handle,
                               const void* options,
                               void* client_data,
                               void* callback) {
    LogCallEvent("eos_lobby_create", "_EOS_Lobby_CreateLobby@16");
    if (original_lobby_create != nullptr) {
        original_lobby_create(handle, options, client_data, callback);
    }
}

void __stdcall HookLobbyJoin(EosHandle handle,
                             const void* options,
                             void* client_data,
                             void* callback) {
    LogCallEvent("eos_lobby_join", "_EOS_Lobby_JoinLobby@16");
    if (original_lobby_join != nullptr) {
        original_lobby_join(handle, options, client_data, callback);
    }
}

void __stdcall HookLobbyLeave(EosHandle handle,
                              const void* options,
                              void* client_data,
                              void* callback) {
    LogCallEvent("eos_lobby_leave", "_EOS_Lobby_LeaveLobby@16");
    if (original_lobby_leave != nullptr) {
        original_lobby_leave(handle, options, client_data, callback);
    }
}

EosResult __stdcall HookLobbyCreateSearch(EosHandle handle,
                                          const void* options,
                                          EosHandle* out_search_handle) {
    EosResult result = -1;
    if (original_lobby_create_search != nullptr) {
        result = original_lobby_create_search(handle, options, out_search_handle);
    }
    LogPointerResultEvent("eos_lobby_create_search",
                          "_EOS_Lobby_CreateLobbySearch@12",
                          out_search_handle == nullptr ? nullptr : *out_search_handle);
    return result;
}

void __stdcall HookLobbySearchFind(EosHandle handle,
                                   const void* options,
                                   void* client_data,
                                   void* callback) {
    LogCallEvent("eos_lobby_search_find", "_EOS_LobbySearch_Find@16");
    if (original_lobby_search_find != nullptr) {
        original_lobby_search_find(handle, options, client_data, callback);
    }
}

EosResult __stdcall HookSendPacket(EosHandle handle,
                                   const EosP2PSendPacketOptions* options) {
    const auto before = NowMicros();
    SleepIfConfigured(delay_send_ms);

    EosResult result = -1;
    if (original_send_packet != nullptr) {
        result = original_send_packet(handle, options);
    }

    int channel = -1;
    std::uint32_t bytes = 0;
    std::int32_t reliability = -1;
    std::int32_t delayed_delivery = -1;
    const char* socket_name = "";
    if (options != nullptr) {
        channel = static_cast<int>(options->channel);
        bytes = options->data_length_bytes;
        reliability = options->reliability;
        delayed_delivery = options->allow_delayed_delivery;
        socket_name = SocketName(options->socket_id);
    }

    LogSendEvent(
        before, result, channel, bytes, reliability, delayed_delivery, socket_name);
    return result;
}

EosResult __stdcall HookReceivePacket(EosHandle handle,
                                      const EosP2PReceivePacketOptions* options,
                                      EosProductUserId* out_peer_id,
                                      EosP2PSocketId* out_socket_id,
                                      std::uint8_t* out_channel,
                                      void* out_data,
                                      std::uint32_t* out_bytes_written) {
    const auto before = NowMicros();
    SleepIfConfigured(delay_recv_ms);

    EosResult result = -1;
    if (original_receive_packet != nullptr) {
        result = original_receive_packet(handle,
                                         options,
                                         out_peer_id,
                                         out_socket_id,
                                         out_channel,
                                         out_data,
                                         out_bytes_written);
    }

    const int channel = result == kEosSuccess ? ChannelOrAny(out_channel) : -1;
    const std::uint32_t bytes =
        result == kEosSuccess && out_bytes_written != nullptr
            ? *out_bytes_written
            : 0;
    const std::uint32_t max_bytes =
        options == nullptr ? 0 : options->max_data_size_bytes;
    LogReceiveEvent(before,
                    result,
                    channel,
                    bytes,
                    max_bytes,
                    result == kEosSuccess ? SocketName(out_socket_id) : "");
    return result;
}

EosResult __stdcall HookGetNextReceivedPacketSize(
    EosHandle handle,
    const EosP2PGetNextReceivedPacketSizeOptions* options,
    std::uint32_t* out_packet_size_bytes) {
    const auto before = NowMicros();

    EosResult result = -1;
    if (original_next_size != nullptr) {
        result = original_next_size(handle, options, out_packet_size_bytes);
    }

    const std::uint32_t bytes =
        result == kEosSuccess && out_packet_size_bytes != nullptr
            ? *out_packet_size_bytes
            : 0;
    LogNextSizeEvent(before, result, bytes);
    return result;
}

void __stdcall HookPlatformTick(EosHandle handle) {
    if (original_platform_tick != nullptr) {
        original_platform_tick(handle);
    }
}

bool __thiscall HookSteamSendP2PPacket(void* self,
                                       std::uint64_t remote,
                                       const void* data,
                                       std::uint32_t bytes,
                                       int send_type,
                                       int channel) {
    const auto before = NowMicros();
    bool bridge_result = false;
    if (IsBridgeEnabled()) {
        bridge_result = SendBridgePacket(BridgePacket{
            remote,
            0,
            channel,
            send_type,
            bytes,
            data,
        });
    }

    bool result = false;
    if (GetBridgeMode() == BridgeMode::Replace) {
        result = bridge_result;
    } else if (original_steam_send_p2p_packet != nullptr) {
        result = original_steam_send_p2p_packet(
            self, remote, data, bytes, send_type, channel);
    }
    LogSteamP2PSendEvent(before, result, remote, bytes, send_type, channel);
    if (result) {
        MaybeLogSteamSessionState(self, remote, before);
    }
    return result;
}

bool __thiscall HookSteamIsP2PPacketAvailable(void* self,
                                              std::uint32_t* bytes,
                                              int channel) {
    const auto before = NowMicros();
    if (HasBridgePacket(channel, bytes)) {
        LogSteamP2PAvailableEvent(
            before, true, bytes == nullptr ? 0 : *bytes, channel);
        return true;
    }

    bool result = false;
    if ((GetBridgeMode() != BridgeMode::Replace || ShouldFallbackToSteam()) &&
        original_steam_is_p2p_packet_available != nullptr) {
        result = original_steam_is_p2p_packet_available(self, bytes, channel);
    }
    if (result) {
        LogSteamP2PAvailableEvent(
            before, result, bytes == nullptr ? 0 : *bytes, channel);
    } else {
        const auto previous = last_steam_available_false_log_us.load();
        if (before - previous >= 250'000) {
            last_steam_available_false_log_us.store(before);
            LogSteamP2PAvailableEvent(before, false, 0, channel);
        }
    }
    return result;
}

bool __thiscall HookSteamReadP2PPacket(void* self,
                                       void* destination,
                                       std::uint32_t max_bytes,
                                       std::uint32_t* bytes_read,
                                       std::uint64_t* remote,
                                       int channel) {
    const auto before = NowMicros();
    if (ReadBridgePacket(channel, destination, max_bytes, bytes_read, remote)) {
        const std::uint64_t peer = remote == nullptr ? 0 : *remote;
        LogSteamP2PReadEvent(before,
                             true,
                             peer,
                             bytes_read == nullptr ? 0 : *bytes_read,
                             max_bytes,
                             channel);
        MaybeLogSteamSessionState(self, peer, before);
        return true;
    }

    bool result = false;
    if ((GetBridgeMode() != BridgeMode::Replace || ShouldFallbackToSteam()) &&
        original_steam_read_p2p_packet != nullptr) {
        result = original_steam_read_p2p_packet(
            self, destination, max_bytes, bytes_read, remote, channel);
    }
    if (result) {
        const std::uint64_t peer = remote == nullptr ? 0 : *remote;
        LogSteamP2PReadEvent(before,
                             result,
                             peer,
                             bytes_read == nullptr ? 0 : *bytes_read,
                             max_bytes,
                             channel);
        MaybeLogSteamSessionState(self, peer, before);
    }
    return result;
}

bool __thiscall HookSteamGetP2PSessionState(void* self,
                                            std::uint64_t remote,
                                            void* state) {
    const auto before = NowMicros();
    bool result = false;
    if (original_steam_get_p2p_session_state != nullptr) {
        result = original_steam_get_p2p_session_state(self, remote, state);
    }

    if (state != nullptr) {
        const auto* typed_state =
            reinterpret_cast<const SteamP2PSessionState006*>(state);
        LogSteamP2PSessionStateEvent(before,
                                     result,
                                     remote,
                                     typed_state->connection_active,
                                     typed_state->connecting,
                                     typed_state->session_error,
                                     typed_state->using_relay,
                                     typed_state->bytes_queued_for_send,
                                     typed_state->packets_queued_for_send);
    }
    return result;
}

void PatchSteamVtableSlot(void** vtable,
                          int index,
                          void* replacement,
                          void** original,
                          const char* symbol) {
    if (vtable == nullptr || replacement == nullptr || original == nullptr) {
        return;
    }

    void** slot = &vtable[index];
    DWORD old_protect = 0;
    if (!VirtualProtect(slot, sizeof(void*), PAGE_EXECUTE_READWRITE, &old_protect)) {
        LogPatchEvent(symbol, "vtable_protect_failed");
        return;
    }

    if (*original == nullptr) {
        *original = *slot;
    }
    *slot = replacement;

    DWORD unused = 0;
    VirtualProtect(slot, sizeof(void*), old_protect, &unused);
    FlushInstructionCache(GetCurrentProcess(), slot, sizeof(void*));
    LogPatchEvent(symbol, "vtable_patched");
}

void InstallSteamNetworking006Hooks(void* interface_ptr) {
    if (interface_ptr == nullptr) {
        return;
    }

    bool expected = false;
    if (!steam_networking_hooked.compare_exchange_strong(expected, true)) {
        return;
    }

    auto** vtable = *reinterpret_cast<void***>(interface_ptr);
    PatchSteamVtableSlot(vtable,
                         0,
                         reinterpret_cast<void*>(&HookSteamSendP2PPacket),
                         reinterpret_cast<void**>(&original_steam_send_p2p_packet),
                         "ISteamNetworking006::SendP2PPacket");
    PatchSteamVtableSlot(
        vtable,
        1,
        reinterpret_cast<void*>(&HookSteamIsP2PPacketAvailable),
        reinterpret_cast<void**>(&original_steam_is_p2p_packet_available),
        "ISteamNetworking006::IsP2PPacketAvailable");
    PatchSteamVtableSlot(vtable,
                         2,
                         reinterpret_cast<void*>(&HookSteamReadP2PPacket),
                         reinterpret_cast<void**>(&original_steam_read_p2p_packet),
                         "ISteamNetworking006::ReadP2PPacket");
    PatchSteamVtableSlot(vtable,
                         6,
                         reinterpret_cast<void*>(&HookSteamGetP2PSessionState),
                         reinterpret_cast<void**>(&original_steam_get_p2p_session_state),
                         "ISteamNetworking006::GetP2PSessionState");
}

void MaybeLogSteamSessionState(void* self,
                               std::uint64_t remote,
                               std::int64_t now_us) {
    if (self == nullptr || remote == 0 ||
        original_steam_get_p2p_session_state == nullptr) {
        return;
    }

    const auto previous = last_steam_session_state_log_us.load();
    if (now_us - previous < 250'000) {
        return;
    }
    last_steam_session_state_log_us.store(now_us);

    SteamP2PSessionState006 state = {};
    const bool result =
        original_steam_get_p2p_session_state(self, remote, &state);
    LogSteamP2PSessionStateEvent(now_us,
                                 result,
                                 remote,
                                 state.connection_active,
                                 state.connecting,
                                 state.session_error,
                                 state.using_relay,
                                 state.bytes_queued_for_send,
                                 state.packets_queued_for_send);
}

void* __cdecl HookSteamFindOrCreateUserInterface(std::int32_t steam_user,
                                                 const char* version) {
    void* result = nullptr;
    if (original_steam_find_user_interface != nullptr) {
        result = original_steam_find_user_interface(steam_user, version);
    }
    LogSteamInterfaceEvent(version == nullptr ? "" : version, result);
    if (version != nullptr && std::strcmp(version, "SteamNetworking006") == 0) {
        InstallSteamNetworking006Hooks(result);
    }
    return result;
}

void __cdecl HookSteamRunCallbacks() {
    bool expected = false;
    if (steam_callbacks_logged.compare_exchange_strong(expected, true)) {
        LogCallEvent("steam_run_callbacks_seen", "SteamAPI_RunCallbacks");
    }
    const auto now_us = NowMicros();
    const auto previous = last_steam_callbacks_log_us.load();
    if (now_us - previous >= 1'000'000) {
        last_steam_callbacks_log_us.store(now_us);
        LogCallEvent("steam_run_callbacks", "SteamAPI_RunCallbacks");
    }
    if (original_steam_run_callbacks != nullptr) {
        original_steam_run_callbacks();
    }
}

}  // namespace

bool InstallEosHooks() {
    bool expected = false;
    if (!installed.compare_exchange_strong(expected, true)) {
        return true;
    }

    delay_send_ms = std::max(0, ReadEnvInt(L"ISAAC_EOS_PROBE_DELAY_SEND_MS", 0));
    delay_recv_ms = std::max(0, ReadEnvInt(L"ISAAC_EOS_PROBE_DELAY_RECV_MS", 0));
    InitializeBridgeFromEnvironment();

    ImportPatch patches[] = {
        {"_EOS_Platform_Create@4",
         reinterpret_cast<void*>(&HookPlatformCreate),
         reinterpret_cast<void**>(&original_platform_create)},
        {"_EOS_Platform_GetP2PInterface@4",
         reinterpret_cast<void*>(&HookGetP2PInterface),
         reinterpret_cast<void**>(&original_get_p2p_interface)},
        {"_EOS_Platform_GetLobbyInterface@4",
         reinterpret_cast<void*>(&HookGetLobbyInterface),
         reinterpret_cast<void**>(&original_get_lobby_interface)},
        {"_EOS_Lobby_CreateLobby@16",
         reinterpret_cast<void*>(&HookLobbyCreate),
         reinterpret_cast<void**>(&original_lobby_create)},
        {"_EOS_Lobby_JoinLobby@16",
         reinterpret_cast<void*>(&HookLobbyJoin),
         reinterpret_cast<void**>(&original_lobby_join)},
        {"_EOS_Lobby_LeaveLobby@16",
         reinterpret_cast<void*>(&HookLobbyLeave),
         reinterpret_cast<void**>(&original_lobby_leave)},
        {"_EOS_Lobby_CreateLobbySearch@12",
         reinterpret_cast<void*>(&HookLobbyCreateSearch),
         reinterpret_cast<void**>(&original_lobby_create_search)},
        {"_EOS_LobbySearch_Find@16",
         reinterpret_cast<void*>(&HookLobbySearchFind),
         reinterpret_cast<void**>(&original_lobby_search_find)},
        {"_EOS_P2P_SendPacket@8",
         reinterpret_cast<void*>(&HookSendPacket),
         reinterpret_cast<void**>(&original_send_packet)},
        {"_EOS_P2P_ReceivePacket@28",
         reinterpret_cast<void*>(&HookReceivePacket),
         reinterpret_cast<void**>(&original_receive_packet)},
        {"_EOS_P2P_GetNextReceivedPacketSize@12",
         reinterpret_cast<void*>(&HookGetNextReceivedPacketSize),
         reinterpret_cast<void**>(&original_next_size)},
        {"_EOS_Platform_Tick@4",
         reinterpret_cast<void*>(&HookPlatformTick),
         reinterpret_cast<void**>(&original_platform_tick)},
    };

    const bool ok = PatchImportAddressTable(GetModuleHandleW(nullptr),
                                            "EOSSDK-Win32-Shipping.dll",
                                            patches,
                                            sizeof(patches) / sizeof(patches[0]));

    ImportPatch steam_patches[] = {
        {"SteamInternal_FindOrCreateUserInterface",
         reinterpret_cast<void*>(&HookSteamFindOrCreateUserInterface),
         reinterpret_cast<void**>(&original_steam_find_user_interface)},
        {"SteamAPI_RunCallbacks",
         reinterpret_cast<void*>(&HookSteamRunCallbacks),
         reinterpret_cast<void**>(&original_steam_run_callbacks)},
    };

    const bool steam_ok = PatchImportAddressTable(
        GetModuleHandleW(nullptr),
        "steam_api.dll",
        steam_patches,
        sizeof(steam_patches) / sizeof(steam_patches[0]));

    if (!ok && !steam_ok) {
        installed = false;
    }
    return ok || steam_ok;
}

void ShutdownEosHooks() {
    installed = false;
    ShutdownBridge();
}

}  // namespace eos_probe
