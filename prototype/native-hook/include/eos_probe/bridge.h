#pragma once

#include <cstdint>

namespace eos_probe {

enum class BridgeMode {
    Off,
    Mirror,
    Replace,
};

struct BridgePacket {
    std::uint64_t peer = 0;
    std::uint32_t sequence = 0;
    int channel = 0;
    int send_type = 0;
    std::uint32_t size = 0;
    const void* data = nullptr;
};

void InitializeBridgeFromEnvironment();
void ShutdownBridge();
BridgeMode GetBridgeMode();
bool IsBridgeEnabled();
bool ShouldFallbackToSteam();

bool SendBridgePacket(const BridgePacket& packet);
bool HasBridgePacket(int channel, std::uint32_t* out_size);
bool ReadBridgePacket(int channel,
                      void* destination,
                      std::uint32_t max_size,
                      std::uint32_t* out_size,
                      std::uint64_t* out_peer);

}  // namespace eos_probe
