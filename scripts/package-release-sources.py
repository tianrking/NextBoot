#!/usr/bin/env python3
"""Package exact NextBoot and pinned upstream source archives alongside binaries."""
import argparse
import hashlib
import re
import shutil
import subprocess
from pathlib import Path
import urllib.request
from runtime_assets import PROJECT_DIR, manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version', required=True)
    parser.add_argument('--output', type=Path, default=PROJECT_DIR / 'target/release-media')
    parser.add_argument('--upstream-source', type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+(?:[-.][0-9A-Za-z.-]+)?', args.version):
        parser.error('invalid release version')
    config = manifest()
    args.output.mkdir(parents=True, exist_ok=True)
    upstream = args.output / f'ventoy-{config["version"]}-source.tar.gz'
    temporary = upstream.with_suffix('.download')
    try:
        if args.upstream_source:
            shutil.copyfile(args.upstream_source, temporary)
        else:
            with urllib.request.urlopen(config['source_archive'], timeout=120) as response, temporary.open('wb') as output:
                shutil.copyfileobj(response, output)
        with temporary.open('rb') as source_file:
            digest = hashlib.file_digest(source_file, 'sha256').hexdigest()
        if digest != config['source_sha256']:
            raise ValueError('upstream source archive checksum mismatch')
        temporary.replace(upstream)
    finally:
        temporary.unlink(missing_ok=True)
    subprocess.run(['git', 'diff', '--quiet', 'HEAD', '--'], cwd=PROJECT_DIR, check=True)
    own_source = args.output / f'nextboot-{args.version}-source.tar.gz'
    subprocess.run(['git', 'archive', '--format=tar.gz', '--output', str(own_source.resolve()), 'HEAD'], cwd=PROJECT_DIR, check=True)
    for path in (own_source, upstream):
        print(f'Packaged source: {path.name}')


if __name__ == '__main__':
    main()
