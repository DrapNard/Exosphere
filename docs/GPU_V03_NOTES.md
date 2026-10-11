# GPU continuation v0.3

**Maintainer:** DrapNard

This iteration focuses on GPU memory correctness and safe decoder boundaries rather than claiming unsupported Maxwell semantics.

## Block-linear surfaces

Exosphere models Tegra block-linear memory in byte coordinates. A GOB is 64 bytes wide and 8 rows high (512 bytes). Texture formats are converted to byte/block dimensions before address translation, so BC/ASTC and uncompressed formats share the same tiling core.

## TIC / TSC

TIC and TSC entries are represented losslessly as eight 32-bit words. Exosphere currently promotes only the descriptor-table stride and texture-handle split that are sufficiently documented. Unknown descriptor bitfields remain raw.

## MME policy

The semantic interpreter and the raw decoder are intentionally separate. A raw instruction is not assigned semantics until validated. This allows Exosphere to later consume a native decoder or development-time envydis integration without redesigning the graphics frontend.

## Resource identity

GPU virtual addresses are locators, never resource identities. Every resource access must resolve through `GpuAddressSpace`; canonical identity comes from `ResourceTable`.
