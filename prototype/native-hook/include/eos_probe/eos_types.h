#pragma once

#include <cstdint>

namespace eos_probe {

using EosResult = std::int32_t;
using EosHandle = void*;
using EosProductUserId = void*;

struct EosP2PSocketId {
    std::int32_t api_version;
    char socket_name[33];
};

struct EosP2PSendPacketOptions {
    std::int32_t api_version;
    EosProductUserId local_user_id;
    EosProductUserId remote_user_id;
    const EosP2PSocketId* socket_id;
    std::uint8_t channel;
    std::uint8_t reserved_padding[3];
    std::uint32_t data_length_bytes;
    const void* data;
    std::int32_t allow_delayed_delivery;
    std::int32_t reliability;
    std::int32_t disable_auto_accept_connection;
};

struct EosP2PReceivePacketOptions {
    std::int32_t api_version;
    EosProductUserId local_user_id;
    std::uint32_t max_data_size_bytes;
    const std::uint8_t* requested_channel;
};

struct EosP2PGetNextReceivedPacketSizeOptions {
    std::int32_t api_version;
    EosProductUserId local_user_id;
    const std::uint8_t* requested_channel;
};

using EosP2PSendPacketFn =
    EosResult(__stdcall*)(EosHandle, const EosP2PSendPacketOptions*);

using EosP2PReceivePacketFn =
    EosResult(__stdcall*)(EosHandle,
                          const EosP2PReceivePacketOptions*,
                          EosProductUserId*,
                          EosP2PSocketId*,
                          std::uint8_t*,
                          void*,
                          std::uint32_t*);

using EosP2PGetNextReceivedPacketSizeFn =
    EosResult(__stdcall*)(EosHandle,
                          const EosP2PGetNextReceivedPacketSizeOptions*,
                          std::uint32_t*);

using EosPlatformTickFn = void(__stdcall*)(EosHandle);

using EosPlatformCreateFn = EosHandle(__stdcall*)(const void*);
using EosPlatformGetInterfaceFn = EosHandle(__stdcall*)(EosHandle);

using EosLobbyAsync4Fn =
    void(__stdcall*)(EosHandle, const void*, void*, void*);

using EosLobbyCreateSearchFn =
    EosResult(__stdcall*)(EosHandle, const void*, EosHandle*);

using SteamFindOrCreateUserInterfaceFn =
    void*(__cdecl*)(std::int32_t, const char*);

using SteamRunCallbacksFn = void(__cdecl*)();

using SteamSendP2PPacketFn =
    bool(__thiscall*)(void*, std::uint64_t, const void*, std::uint32_t, int, int);

using SteamIsP2PPacketAvailableFn =
    bool(__thiscall*)(void*, std::uint32_t*, int);

using SteamReadP2PPacketFn =
    bool(__thiscall*)(void*, void*, std::uint32_t, std::uint32_t*, std::uint64_t*, int);

using SteamGetP2PSessionStateFn =
    bool(__thiscall*)(void*, std::uint64_t, void*);

struct SteamP2PSessionState006 {
    std::uint8_t connection_active;
    std::uint8_t connecting;
    std::uint8_t session_error;
    std::uint8_t using_relay;
    std::int32_t bytes_queued_for_send;
    std::int32_t packets_queued_for_send;
    std::uint32_t remote_ip;
    std::uint16_t remote_port;
};

}  // namespace eos_probe
