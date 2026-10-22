#!/usr/bin/env bash
# Prepared on Windows; must be executed and verified with Apple's signed-in download.
set -euo pipefail
cd "$(dirname "$0")/.."
msc="${MSC:-metal-shaderconverter}"
mkdir -p tools/metal
"$msc" --version | tee tools/metal/converter-version.txt
"$msc" --help > tools/metal/converter-help.txt
for entry in vertexMain fragmentMain computeMain; do
    "$msc" "cooked/$entry.dxil" \
        --root-signature mac/root-signature.json \
        --minimum-os-build-version 26.0 \
        --rename-entry-point "$entry" \
        --output-reflection-file "tools/metal/$entry.json" \
        -o "tools/metal/$entry.metallib"
done
