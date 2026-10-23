# Benchmark media

Poly Haven **Coast Rocks 01**, photogrammetry model and diffuse/normal/ARM maps:
https://polyhaven.com/a/coast_rocks_01

License: CC0 1.0 Universal, https://polyhaven.com/license and
https://creativecommons.org/publicdomain/zero/1.0/ . No asset files are committed.
`fetch-scene.ps1` fetches the publisher's glTF, binary and JPG files on demand,
checks the MD5 values published by https://api.polyhaven.com/files/coast_rocks_01,
and stores them under ignored `captures/assets`. MD5 here detects download drift;
it is not a signature or security boundary. Ground geometry is original code.

Comparison image: Epic's Nanite city example, viewed for comparison only:
https://dev.epicgames.com/documentation/en-us/unreal-engine/nanite-virtualized-geometry-in-unreal-engine
No Epic media is redistributed with the benchmark.
