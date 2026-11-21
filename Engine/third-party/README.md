# Native dependencies

- `bullet-cpu-3.25.tar.gz` contains the original LinearMath, BulletCollision and BulletDynamics C++ sources and headers from [Bullet 3.25](https://github.com/bulletphysics/bullet3/releases/tag/3.25). It includes the upstream license. The GPU backend, examples and tools are outside this archive.
- `miniaudio-0.11.23.h.gz` is the original [miniaudio 0.11.23 header](https://github.com/mackron/miniaudio/tree/0.11.23), compressed without source changes. Tellurium uses it under MIT No Attribution.

- `tracy-client-0.12.2.tar.gz` contains the unmodified `public/` client sources from [Tracy 0.12.2](https://github.com/wolfpld/tracy/releases/tag/v0.12.2). See `tracy.LICENSE.txt`.

Cargo extracts the vendored dependencies into its build directories and compiles them with the host C/C++ toolchain. The Tracy client is compiled with the profiling feature.
