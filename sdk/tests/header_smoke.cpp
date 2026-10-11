#include "exosphere_device_plugin.h"
#include <type_traits>
static_assert(std::is_standard_layout_v<ExosphereDeviceHostApiV1>);
static_assert(std::is_standard_layout_v<ExosphereDevicePluginV1>);
int main() { return EXOSPHERE_DEVICE_ABI_VERSION == 1u ? 0 : 1; }
