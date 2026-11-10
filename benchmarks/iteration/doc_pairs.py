"""Optional exact documentation-extraction comparison; no compiler timing claim."""
import argparse
import hashlib
import json
from pathlib import Path
import random
import subprocess


def corpus():
    """Valid XML plus deterministic token/character damage around wrapper and text boundaries."""
    summaries = ['', 'Current amount.', 'one\t two\r\nthree', 'café 世界 🚀', 'a &amp; b', '&lt;7',
                 '<![CDATA[a < b]]>', '<c>Amount</c>', '<see langword="null"/>', '<b>nested <i>text</i></b>',
                 '<paramref name="value"/>', 'raw ]]> terminator', 'unknown &missing;', '\x00', '\u00a0']
    roots = ['<member>{}</member>', '<member name="T:Game.State">{}</member>',
             '<member name="T:Game&amp;State">{}</member>', '<member name="\"">{}</member>',
             '<member xmlns="urn:other">{}</member>', '<other>{}</other>', '{}']
    tags = ['<summary>{}</summary>', '<summary class="detail">{}</summary>',
            '<summary xmlns="urn:other">{}</summary>', '<summary>{}</summary><remarks>Extra.</remarks>',
            '<summary>{}</summary><summary>Later.</summary>', '<summary>{}</summary><broken>',
            '<param name="value">{}</param>']
    seeds = [root.format(tag.format(text)) for root in roots for tag in tags for text in summaries]
    rows = [None, '', *seeds]
    for seed in seeds[:40]:
        for offset in range(len(seed) + 1):
            rows.extend([seed[:offset] + '<' + seed[offset:], seed[:offset] + '&' + seed[offset:],
                         seed[:offset] + seed[offset + 1:]])
    plain = '<member name="T:Game.State">\n<summary>Current amount.</summary>\n</member>'
    for offset in [0, 14, 20, 37, 45, len(plain) - 1, len(plain)]:
        for value in range(256):
            rows.append(plain[:offset] + chr(value) + plain[offset:])
    rng = random.Random(684)
    for _ in range(4000):
        seed = rng.choice(seeds)
        start = rng.randrange(len(seed) + 1)
        end = rng.randrange(start, len(seed) + 1)
        rows.append(seed[:start] + rng.choice(['"', "'", '>', '</member>', '<!-- -->', ' ', '\r\n', '\u2028', '🚀']) + seed[end:])
    return rows


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def closure(path):
    return {str(p.relative_to(path)): sha(p) for p in sorted(path.rglob('*')) if p.is_file()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ['baseline', 'candidate', 'helper', 'dotnet-host', 'output']:
        parser.add_argument('--' + arg, type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    inputs = args.output / 'corpus.json'
    inputs.write_text(json.dumps(corpus(), ensure_ascii=True) + '\n')
    directories = dict(baseline=args.baseline.resolve(), candidate=args.candidate.resolve(), helper=args.helper.resolve().parent)
    frozen = {arm: closure(path) for arm, path in directories.items()}
    receipt = dict(passed=False, timings_accepted=False, sdk_sha256=frozen, corpus_sha256=sha(inputs), arms={})
    try:
        reference = None
        for arm in ['baseline', 'candidate']:
            command = [str(args.dotnet_host.resolve()), str(args.helper.resolve()), str(directories[arm]), str(inputs.resolve())]
            process = subprocess.run(command, capture_output=True, timeout=90)
            stdout = args.output / (arm + '.json')
            stdout.write_bytes(process.stdout)
            (args.output / (arm + '.stderr')).write_bytes(process.stderr)
            if process.returncode or process.stderr:
                raise RuntimeError(f'{arm} documentation probe failed: {process.returncode}')
            result = json.loads(process.stdout)
            receipt['arms'][arm] = dict(command=command, cases=result['cases'], output_sha256=result['outputSha256'], raw_sha256=sha(stdout))
            if result['cases'] != len(corpus()) or result['corpusSha256'] != sha(inputs):
                raise RuntimeError('corpus identity/count mismatch')
            if reference is None:
                reference = result['output']
            elif result['output'] != reference:
                receipt['mismatches'] = [i for i, (before, after) in enumerate(zip(reference, result['output'])) if before != after]
                raise RuntimeError('exact documentation text differs')
        if any(closure(path) != frozen[arm] for arm, path in directories.items()):
            raise RuntimeError('frozen SDK/helper files changed')
        receipt['passed'] = True
    except BaseException as error:
        receipt['failure'] = repr(error)
        raise
    finally:
        (args.output / 'result.json').write_text(json.dumps(receipt, indent=2) + '\n')


if __name__ == '__main__':
    main()
