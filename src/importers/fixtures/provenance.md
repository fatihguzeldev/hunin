# Recorded importer controls

`native.json` and `native.syms.json` are unmodified samply 0.13.1 output from a
local macOS ARM64 C CPU control on 2026-10-23. The recording contains 334 samples.
The symbol sidecar resolves `hunin_cpu_hotspot` and its measured source location.
The raw source path is historical evidence, not a claim that the file currently
at that path is identical to the measured source. The portable fixture in
`src/collection/fixtures/native_cpu.c` has since gained a compiler-portability macro.

`graal.json` is unmodified GraalPy 3.13.14 / GraalVM CE Native 25.4.4 CPUSampler
output from `collectors/python/fixtures/cpu_work.py`, using `GatherHitTimes=true`.
It contains seven self-hit epoch timestamps and the known Python function.

`go.pprof` is the unmodified Go 1.27.3 CPU profile from the uninstrumented
`collectors/go/fixtures` test, recorded with `go test -cpuprofile`. The independent
`go tool pprof -top` reports 250 ms self CPU in `huninGoHotspot` of 270 ms total
sampled CPU. The importer preserves aggregate CPU weights in microseconds.

These controls exercise format normalization, units, source/function resolution,
stable replay identity and schema acceptance. They do not establish collection
on Windows, Linux or other runtime versions.
