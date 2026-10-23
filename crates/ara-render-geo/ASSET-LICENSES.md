# Benchmark media

Poly Haven **Coast Rocks 01**, photogrammetry model and diffuse/normal/ARM maps:
https://polyhaven.com/a/coast_rocks_01

License: CC0 1.0 Universal, https://polyhaven.com/license and
https://creativecommons.org/publicdomain/zero/1.0/ . No asset files are committed.
`fetch-scene.ps1` fetches the publisher's glTF, binary and JPG files on demand,
checks the MD5 values published by https://api.polyhaven.com/files/coast_rocks_01,
and stores them under ignored `captures/assets`. MD5 here detects download drift;
it is not a signature or security boundary. Ground geometry is original code.

Comparison image: Epic's Nanite rocky canyon from its UE5 Early Access article:
https://www.unrealengine.com/blog/unreal-engine-5-is-now-available-in-early-access?lang=en-US
The report reproduces a reduced image solely for visual comparison, credited to
Epic Games. It is copyrighted Epic media, not a CC0 asset or benchmark input.
No Unreal geometry, textures or implementation are included in the engine.
