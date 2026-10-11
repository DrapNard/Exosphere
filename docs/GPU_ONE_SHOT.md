# GPU one-shot baseline

**Maintainer:** DrapNard  
**Status:** structural GPU stack feature-complete for v0.1; semantic compatibility coverage continues.

## End-to-end path

```text
GPFIFO / pushbuffer
    ↓
PFIFO packet decoder
    ↓
subchannel + SetObject router
    ↓
┌───────────┬───────────┬──────────┬──────────┬──────────┐
│ MAXWELL_B │ COMPUTE_B │ INLINE2M │ FERMI2D  │ DMA COPY │
└─────┬─────┴─────┬─────┴────┬─────┴────┬─────┴────┬─────┘
      ↓           ↓          ↓          ↓          ↓
 state/events   QMD       upload      2D state   transfers
      └───────────┬──────────┴──────────┬──────────┘
                  ↓                     ↓
             canonical GPU IR ← resource/sync state
                  ↓
           dependency graph / cache
                  ↓
             Shader discovery
                  ↓
         Maxwell SM53 structural IR
                  ↓
           semantic Shader IR
                  ↓
                SPIR-V
                  ↓
          Vulkan execution plan
                  ↓
       Linux Vulkan device / queue
```

## Implemented structural coverage

| Area | v0.1 status |
|---|---|
| PFIFO packet modes | implemented |
| Subchannel/class routing | implemented |
| MAXWELL_B state | implemented baseline |
| Render targets | tracked |
| Viewports/scissors | tracked |
| Pipeline shader slots | tracked |
| Vertex-array draw | semantic event |
| Index-buffer state/draw | tracked/event |
| MME upload/calls | tracked + program extraction |
| MAXWELL_COMPUTE_B | state + QMD submission boundary |
| QMD 01.07 | dispatch/program/CB parser baseline |
| INLINE_TO_MEMORY | state + inline words |
| FERMI_TWOD_A | source/destination/operation/raw register state |
| MAXWELL_DMA_COPY_A | address/pitch/shape/remap/block state |
| Shader program discovery | program-region + pipeline slots |
| Graphics shader header | 0x50 retained |
| Scheduler words | separated from instructions |
| Maxwell ISA unknowns | preserved losslessly |
| GPU dependency graph | implemented baseline |
| Portable GPU cache keys | BLAKE3 |
| SPIR-V backend | portable known shader subset |
| Vulkan instance/device/queue | implemented |
| Vulkan offscreen graphics | real draw + readback smoke path |

## Why this is not labelled “100% game compatibility”

The architecture has a place for every major Switch GPU engine and a complete path to the host GPU,
but the entire SM53 shader ISA, every MME opcode, every rare MAXWELL_B register and all exact tiling /
compression formats are not yet semantically translated. Unknown behavior is deliberately represented as
opaque state rather than guessed.

This distinction is important: v0.1 is now **GPU-stack complete**, not **GPU-knowledge complete**.

## Next compatibility work

1. Grow SM53 semantic instruction coverage from traces/tests and vetted upstream references.
2. Add MME semantic interpreter/AOT translation.
3. Decode exact render-target/texture formats and block-linear layouts.
4. Build descriptor/texture/sampler translation.
5. Translate indexed/instanced/indirect draws and production synchronization.
6. Parse QMD constant buffers and bind compute resources.
7. Add presentation/swapchain in the Linux platform layer.
8. Add Metal and OpenGL compatibility backends without changing canonical GPU IR.
