# Vulkan backend v2 matrix

| Feature | v2 status | Notes |
|---|---|---|
| Instance/device/queue | Native | Vulkan 1.3 |
| synchronization2 | Native | requested at device creation |
| Dynamic viewport/scissor | Native | |
| Vertex buffers | Native core | canonical layout added |
| Index buffers | Native core | U8 requires host support |
| Direct draw | Native core | requires prepared pipeline |
| Indexed draw | Native core | requires prepared pipeline |
| Draw indirect | Native core | |
| Dispatch | Native core | requires prepared compute pipeline |
| Dispatch indirect | Native core | |
| Buffer copy | Native | |
| Buffer fill | Native | |
| Guest buffer writeback | Native | host-visible coherent baseline |
| Image allocation/views | Native | device-local VkImage + VkImageView |
| BCn/ASTC Vulkan formats | Mapped | actual host format support checked later |
| Block-linear guest data | Native conversion | Tegra X1 deswizzle for single-mip payloads; multi-mip offsets remain frontend metadata |
| Samplers | Native baseline | TSC semantic promotion still improves fidelity |
| Pipeline layout descriptors | Native schema | descriptor population still being wired |
| Push constants | Native schema | |
| Sampled descriptors | Native | pool/set allocation and writes |
| Storage descriptors | Native | buffer/image descriptor writes |
| Dynamic rendering attachments | Native | ash Vulkan 1.3 path |
| Image upload | Native baseline | staging buffer -> image, linear canonical source |
| Image copy/blit/resolve | Native core | Vk copy/blit/resolve commands with layout tracking |
| Image clear | Native core | color + depth/stencil |
| Queries/timestamps | Native | occlusion, timestamp, pipeline statistics; XFB query extension-gated |
| Conditional rendering | Native when available | VK_EXT_conditional_rendering negotiated per device |
| Transform feedback | Native when available | VK_EXT_transform_feedback negotiated per device |
| Swapchain/presentation | Platform layer | `VulkanPresentation` contract added |
