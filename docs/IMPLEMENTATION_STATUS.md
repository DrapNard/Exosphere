
### Maxwell v4.7 — unified semantic pipeline
Coverage and compilation now share `decode_semantic()` + `lower_semantic()`. Coverage is measured after canonical lowering, preventing recognized-but-opaque instructions from being reported as supported. Common ALU forms, memory, atomics, textures, IPA, SHFL, SYS, barriers, direct/indirect flow and reconvergence metadata are connected to canonical Shader IR. Remaining work is long-tail exactness and real-corpus differential validation.
