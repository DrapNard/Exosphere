/* SPDX-License-Identifier: GPL-3.0-or-later
 * Project: Exosphere
 * Author/Maintainer: DrapNard
 */
#pragma once
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define EXOSPHERE_DEVICE_ABI_VERSION 1u

typedef enum ExosphereStatus {
    EXOSPHERE_OK = 0,
    EXOSPHERE_INVALID_ARGUMENT = -1,
    EXOSPHERE_UNSUPPORTED = -2,
    EXOSPHERE_NOT_FOUND = -3,
    EXOSPHERE_HOST_ERROR = -4,
    EXOSPHERE_ABI_MISMATCH = -5,
    EXOSPHERE_INTERNAL = -127
} ExosphereStatus;

/** Broad guest-device class used for discovery, configuration and UI. */
typedef enum ExosphereDeviceClass {
    EXOSPHERE_DEVICE_KEYBOARD = 1,
    EXOSPHERE_DEVICE_MOUSE = 2,
    EXOSPHERE_DEVICE_GAMEPAD = 3,
    EXOSPHERE_DEVICE_STORAGE = 4,
    EXOSPHERE_DEVICE_NETWORK = 5,
    EXOSPHERE_DEVICE_SENSOR = 6,
    EXOSPHERE_DEVICE_AUDIO = 7,
    EXOSPHERE_DEVICE_DISPLAY = 8,
    EXOSPHERE_DEVICE_INPUT_COMPOSITE = 9,
    EXOSPHERE_DEVICE_CUSTOM = 0xffff
} ExosphereDeviceClass;

/** Stable event discriminator used by ExosphereDeviceEventV1::event_type. */
typedef enum ExosphereDeviceEventType {
    EXOSPHERE_DEVICE_EVENT_KEY = 1,
    EXOSPHERE_DEVICE_EVENT_MOUSE_MOTION = 2,
    EXOSPHERE_DEVICE_EVENT_MOUSE_BUTTON = 3,
    EXOSPHERE_DEVICE_EVENT_MOUSE_WHEEL = 4
} ExosphereDeviceEventType;

/** Normalized keyboard event.
 *
 * `usage` uses USB HID usage codes rather than Linux evdev, Windows virtual-key, or Cocoa codes.
 * Host plugins are responsible for mapping their native key representation into this stable value.
 */
typedef struct ExosphereKeyboardEventV1 {
    uint32_t usage;
    uint8_t pressed;
    uint8_t repeat;
    uint8_t reserved[2];
} ExosphereKeyboardEventV1;

/** Relative pointer motion. */
typedef struct ExosphereMouseMotionEventV1 {
    int32_t dx;
    int32_t dy;
} ExosphereMouseMotionEventV1;

/** Normalized mouse button transition. */
typedef struct ExosphereMouseButtonEventV1 {
    uint32_t button;
    uint8_t pressed;
    uint8_t reserved[3];
} ExosphereMouseButtonEventV1;

/** High-resolution wheel delta; 120 units represent one conventional detent. */
typedef struct ExosphereMouseWheelEventV1 {
    int32_t x;
    int32_t y;
} ExosphereMouseWheelEventV1;

/** One host-to-guest device event.
 *
 * `payload` is borrowed and valid only until the receiving callback returns. Consumers must copy
 * data synchronously if they need to keep it.
 */
typedef struct ExosphereDeviceEventV1 {
    uint32_t event_type;
    uint32_t flags;
    uint64_t timestamp_ns;
    const uint8_t* payload;
    size_t payload_len;
} ExosphereDeviceEventV1;

/** Host callbacks available to a hypervisor device plugin.
 *
 * ABI NOTE: Plugins must check `abi_version` and may only access members whose offsets are within
 * `struct_size`. Future ABI revisions append fields rather than reordering existing fields.
 */
typedef struct ExosphereDeviceHostApiV1 {
    uint32_t abi_version;
    uint32_t struct_size;
    void* userdata;

    /** Emits one normalized host-to-guest event.
     * @param userdata Runtime-owned context passed unchanged.
     * @param event Event and payload valid only for the duration of this call.
     * @return EXOSPHERE_OK when accepted; another status when the runtime rejects the event.
     */
    ExosphereStatus (*emit_event)(void* userdata, const ExosphereDeviceEventV1* event);

    /** Requests delivery of a guest-visible interrupt associated with the virtual device.
     * @param userdata Runtime-owned context passed unchanged.
     * @param irq Runtime-defined virtual IRQ number assigned to the device.
     * @return EXOSPHERE_OK when queued or injected.
     */
    ExosphereStatus (*request_irq)(void* userdata, uint32_t irq);
} ExosphereDeviceHostApiV1;

/** Device-plugin vtable.
 *
 * The vtable and `plugin_name` storage remain owned by the dynamic library. `userdata` is entirely
 * plugin-owned and is passed back unchanged to each callback.
 */
typedef struct ExosphereDevicePluginV1 {
    uint32_t abi_version;
    uint32_t struct_size;
    const char* plugin_name_ptr;
    size_t plugin_name_len;
    uint32_t device_class;
    void* userdata;

    /** Initializes host integration. Called exactly once before poll(). */
    ExosphereStatus (*start)(void* userdata, const ExosphereDeviceHostApiV1* host);
    /** Pumps host-side events. Implementations with a worker thread may make this a cheap no-op. */
    ExosphereStatus (*poll)(void* userdata);
    /** Handles guest-to-device controls such as LED state or pointer capture. */
    ExosphereStatus (*control)(void* userdata, uint32_t command, const uint8_t* data, size_t len);
    /** Stops host integration. Must tolerate cleanup after a partial start failure. */
    void (*stop)(void* userdata);
} ExosphereDevicePluginV1;

/** Plugin entrypoint signature.
 *
 * @param requested_abi ABI revision requested by the runtime.
 * @param out_plugin Receives a library-owned immutable vtable pointer.
 * @return EXOSPHERE_OK on success or EXOSPHERE_ABI_MISMATCH when the requested ABI is unsupported.
 */
typedef ExosphereStatus (*ExosphereDevicePluginQueryFn)(uint32_t requested_abi, const ExosphereDevicePluginV1** out_plugin);

#ifdef __cplusplus
}
#endif
