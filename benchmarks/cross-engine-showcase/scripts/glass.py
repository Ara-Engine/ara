"""Transmissive glass in benchmark GLBs: rewrite the JSON chunk in place, BIN untouched.

Default (`volume`): every material with KHR_materials_transmission whose meshes are
closed solids gains KHR_materials_volume with the solids' thickness (the smallest
extent of their local bounds), so engines refract them as Cycles does (Cycles treats a
closed transmissive mesh as a solid medium; glTF without a volume is a thin wall).
Materials without transmission (alpha-blended films included) are never changed.
build_source.py applies it after every export.

`--refraction-test`: additionally turns the material-ball films ("clear glass",
"tinted glass") into real glass (transmission, IOR, volume and attenuation). Only for
a separate test copy: the gate scene must stay what Cycles rendered (alpha films).

Usage: python -I glass.py <scene.glb> [--refraction-test]
Writes <scene.glb>.orig once, before the first rewrite; deterministic output.
"""
import array, json, shutil, struct, sys
from pathlib import Path

# The material-ball films as real glass (--refraction-test): white base (the tint is
# Beer-Lambert absorption, so thick paths colour more), the authored film tint as the
# attenuation colour; the mint ball frosted.
TEST_GLASS = {
    'clear glass': {'roughness': 0.04, 'ior': 1.5, 'attenuationColor': [0.92, 0.95, 0.95], 'attenuationDistance': 0.6},
    'tinted glass': {'roughness': 0.25, 'ior': 1.5, 'attenuationColor': [0.2, 0.55, 0.45], 'attenuationDistance': 1.0},
}
COMPONENTS = {5121: 'B', 5123: 'H', 5125: 'I'}


def read(path):
    data = Path(path).read_bytes()
    magic, version, total = struct.unpack_from('<III', data, 0)
    if magic != 0x46546C67 or version != 2 or total != len(data):
        raise SystemExit(f'{path}: not a glTF 2 binary')
    length, kind = struct.unpack_from('<II', data, 12)
    if kind != 0x4E4F534A:
        raise SystemExit(f'{path}: first chunk is not JSON')
    doc = json.loads(data[20:20 + length])
    rest = data[20 + length:]
    binary = b''
    if len(rest) >= 8:
        bin_length, bin_kind = struct.unpack_from('<II', rest, 0)
        if bin_kind == 0x004E4942:
            binary = rest[8:8 + bin_length]
    return doc, rest, binary


def write(path, doc, rest, backup=True):
    text = json.dumps(doc, separators=(',', ':'), ensure_ascii=False).encode('utf-8')
    text += b' ' * (-len(text) % 4)
    out = struct.pack('<III', 0x46546C67, 2, 20 + len(text) + len(rest)) + struct.pack('<II', len(text), 0x4E4F534A) + text + rest
    path = Path(path)
    original = path.with_name(path.name + '.orig')
    if backup and not original.exists():
        shutil.copy2(path, original)
    temp = path.with_name(path.name + '.tmp')
    temp.write_bytes(out)
    temp.replace(path)


def accessor(doc, binary, index):
    a = doc['accessors'][index]
    view = doc['bufferViews'][a['bufferView']]
    start = view.get('byteOffset', 0) + a.get('byteOffset', 0)
    if a['type'] == 'SCALAR':
        code = COMPONENTS[a['componentType']]
        size = struct.calcsize(code)
        stride = view.get('byteStride', size)
        if stride == size:
            return array.array(code, binary[start:start + a['count'] * size]).tolist()
        return [struct.unpack_from('<' + code, binary, start + i * stride)[0] for i in range(a['count'])]
    stride = view.get('byteStride', 12)
    return [struct.unpack_from('<3f', binary, start + i * stride) for i in range(a['count'])]


def closed(doc, binary, primitive):
    """Whether a triangle primitive is watertight once vertices split at seams are welded."""
    if primitive.get('mode', 4) != 4 or 'indices' not in primitive:
        return False
    positions = accessor(doc, binary, primitive['attributes']['POSITION'])
    weld = {}
    ids = [weld.setdefault(tuple(round(c, 5) for c in p), len(weld)) for p in positions]
    indices = accessor(doc, binary, primitive['indices'])
    edges = {}
    for t in range(0, len(indices) - 2, 3):
        a, b, c = (ids[indices[t]], ids[indices[t + 1]], ids[indices[t + 2]])
        if a == b or b == c or c == a:
            continue  # degenerate (e.g. a UV sphere's pole caps)
        for e in ((a, b), (b, c), (c, a)):
            key = (min(e), max(e))
            edges[key] = edges.get(key, 0) + 1
    return bool(edges) and all(n == 2 for n in edges.values())


def use(doc, extension):
    used = doc.setdefault('extensionsUsed', [])
    if extension not in used:
        used.append(extension)
        used.sort()


def patch(doc, binary, refraction_test=False):
    """Returns {material name: (before, after)} for every material it changed."""
    changed = {}
    primitives = {}
    for mesh in doc.get('meshes', []):
        for p in mesh['primitives']:
            if 'material' in p:
                primitives.setdefault(p['material'], []).append(p)
    for i, m in enumerate(doc.get('materials', [])):
        before = json.loads(json.dumps(m))
        name = m.get('name', '')
        ext = m.setdefault('extensions', {})
        if refraction_test and name in TEST_GLASS:
            spec = TEST_GLASS[name]
            m.pop('alphaMode', None)
            pbr = m.setdefault('pbrMetallicRoughness', {})
            pbr['baseColorFactor'] = [1.0, 1.0, 1.0, 1.0]
            pbr['roughnessFactor'] = spec['roughness']
            pbr['metallicFactor'] = 0.0
            ext['KHR_materials_transmission'] = {'transmissionFactor': 1.0}
            ext['KHR_materials_ior'] = {'ior': spec['ior']}
            ext['KHR_materials_volume'] = {'attenuationColor': spec['attenuationColor'], 'attenuationDistance': spec['attenuationDistance']}
            use(doc, 'KHR_materials_ior')
        if 'KHR_materials_transmission' in ext and 'thicknessFactor' not in ext.get('KHR_materials_volume', {}):
            prims = primitives.get(i, [])
            if prims and all(closed(doc, binary, p) for p in prims):
                bounds = [doc['accessors'][p['attributes']['POSITION']] for p in prims]
                extent = min(min(b['max'][k] - b['min'][k] for k in range(3)) for b in bounds)
                ext.setdefault('KHR_materials_volume', {})['thicknessFactor'] = round(extent, 6)
        if 'KHR_materials_volume' in ext:
            use(doc, 'KHR_materials_volume')
        if 'KHR_materials_transmission' in ext:
            use(doc, 'KHR_materials_transmission')
        if not ext:
            m.pop('extensions')
        if m != before:
            changed[name] = (before, m)
    return changed


def main(argv):
    if not argv or argv[0].startswith('-'):
        raise SystemExit(__doc__)
    path = argv[0]
    doc, rest, binary = read(path)
    changed = patch(doc, binary, '--refraction-test' in argv)
    for name, (before, after) in changed.items():
        print(f'GLASS {name}\n  before {json.dumps(before)}\n  after  {json.dumps(after)}')
    if changed:
        write(path, doc, rest)
    print(f'GLASS_PATCHED {len(changed)} material(s) in {path}')


if __name__ == '__main__':
    main(sys.argv[1:])
