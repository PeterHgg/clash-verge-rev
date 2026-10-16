import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { parseArgs } from 'node:util'

const { values } = parseArgs({
  options: {
    target: { type: 'string' },
    output: { type: 'string' },
    bundle: { type: 'boolean', default: false },
  },
})
const targets = {
  'x86_64-unknown-linux-gnu': { goos: 'linux', goarch: 'amd64' },
  'x86_64-pc-windows-msvc': { goos: 'windows', goarch: 'amd64' },
  'aarch64-apple-darwin': { goos: 'darwin', goarch: 'arm64' },
  'x86_64-apple-darwin': { goos: 'darwin', goarch: 'amd64' },
}
if (values.target !== undefined && !targets[values.target])
  throw new Error(`Unsupported target: ${values.target}`)
const selectedTargets = values.target
  ? [values.target]
  : ['x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc']
const project = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const upstream = 'https://github.com/MetaCubeX/mihomo.git'
const tag = 'v1.19.32'
const commit = '88dcbf7f1614a67c3b36b848ee3592dfa92ada36'
const version = `${tag}-verge-adaptive1`
const toolchain = 'go1.26.8'
const tags = ['with_gvisor']
const patches = [
  {
    name: 'mihomo-manual-health.patch',
    sha256: 'a9cc934801d319b95b5b025dd43b910e149447ef9f173fa647a5b7a0347f00b8',
  },
  {
    name: 'mihomo-adaptive.patch',
    sha256: '72cd844e5f8f44e6ea861288bfd22401a6cff6ce32cfc1d9df4c5fb16a9cd5d6',
  },
]
function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex')
}
for (const patch of patches) {
  if (sha256(join(project, 'scripts', 'patches', patch.name)) !== patch.sha256)
    throw new Error(`Unexpected patch hash: ${patch.name}`)
}
const cache = join(homedir(), '.cache/verge-local-core')
mkdirSync(cache, { recursive: true })
const root = mkdtempSync(join(cache, 'adaptive-'))
const source = join(root, 'source')
const output = values.output ? resolve(values.output) : join(root, 'output')
mkdirSync(output, { recursive: true })

function run(command, args, env = {}, capture = false) {
  const result = spawnSync(command, args, {
    cwd: root,
    env: { ...process.env, ...env },
    stdio: capture ? ['ignore', 'pipe', 'inherit'] : 'inherit',
    encoding: 'utf8',
  })
  if (result.error) throw result.error
  if (result.status !== 0)
    throw new Error(`${command} exited with ${result.status}`)
  return result.stdout?.trim()
}

// Never reuse an existing checkout or the user's unrelated Mihomo fork.
const gitConfig = join(root, 'gitconfig')
writeFileSync(gitConfig, '')
run('git', ['clone', '--depth', '1', '--branch', tag, upstream, source], {
  GIT_CONFIG_GLOBAL: gitConfig,
})
const actualCommit = run('git', ['-C', source, 'rev-parse', 'HEAD'], {}, true)
if (actualCommit !== commit)
  throw new Error(`Unexpected ${tag} commit: ${actualCommit}`)
for (const patch of patches) {
  const path = join(project, 'scripts', 'patches', patch.name)
  run('git', ['-C', source, 'apply', '--check', path])
  run('git', ['-C', source, 'apply', path])
}

const env = {
  GOTOOLCHAIN: toolchain,
  CGO_ENABLED: '0',
  GOAMD64: 'v2',
  GOOS: '',
  GOARCH: '',
  GOMAXPROCS: process.env.GOMAXPROCS || '4',
}
run('go', ['version'], env)
run(
  'go',
  [
    'test',
    '-C',
    source,
    '-p',
    '4',
    '-tags',
    tags.join(','),
    './adapter/outboundgroup',
    './adapter/provider',
    './tunnel',
    '-count=1',
  ],
  env,
)
const sourceEpoch = run(
  'git',
  ['-C', source, 'show', '-s', '--format=%ct', 'HEAD'],
  {},
  true,
)
const buildTime = new Date(
  Number(process.env.SOURCE_DATE_EPOCH || sourceEpoch) * 1000,
).toISOString()
const ldflags = `-extldflags --static -X github.com/metacubex/mihomo/constant.Version=${version} -X github.com/metacubex/mihomo/constant.BuildTime=${buildTime} -w -s -buildid=`
const artifacts = {}
const builds = {}
const hostOS = process.platform === 'win32' ? 'windows' : process.platform
const hostArch = process.arch === 'x64' ? 'amd64' : process.arch
for (const target of selectedTargets) {
  const { goos, goarch } = targets[target]
  const name = `mihomo-${goos}-${goarch}${goarch === 'amd64' ? '-v2' : ''}${goos === 'windows' ? '.exe' : ''}`
  const binary = join(output, name)
  run(
    'go',
    [
      'build',
      '-C',
      source,
      '-p',
      '4',
      '-tags',
      tags.join(','),
      '-trimpath',
      '-ldflags',
      ldflags,
      '-o',
      binary,
      '.',
    ],
    {
      ...env,
      GOOS: goos,
      GOARCH: goarch,
    },
  )
  if (goos === 'darwin' && hostOS === 'darwin') {
    // Tauri preserves this identifier when it re-signs the two sidecar names.
    run('codesign', [
      '--force',
      '--sign',
      '-',
      '--identifier',
      'io.github.clash-verge-rev.mihomo',
      binary,
    ])
  }
  artifacts[name] = sha256(binary)
  let nativeVersion = null
  if (goos === hostOS && goarch === hostArch) {
    nativeVersion = run(binary, ['-v'], {}, true)
    if (!nativeVersion.includes(version))
      throw new Error(
        `Custom version marker missing from ${name}: ${nativeVersion}`,
      )
    console.log(nativeVersion)
  } else {
    console.log(`Skipping native -v verification for cross target ${target}`)
  }
  const sidecars = {}
  if (values.bundle) {
    const sidecarDir = join(project, 'src-tauri', 'sidecar')
    mkdirSync(sidecarDir, { recursive: true })
    for (const core of ['verge-mihomo', 'verge-mihomo-alpha']) {
      const sidecar = `${core}-${target}${goos === 'windows' ? '.exe' : ''}`
      const destination = join(sidecarDir, sidecar)
      copyFileSync(binary, destination)
      chmodSync(destination, statSync(binary).mode & 0o777)
      sidecars[sidecar] = sha256(destination)
      if (sidecars[sidecar] !== artifacts[name])
        throw new Error(`Bundled core hash mismatch: ${destination}`)
      console.log(`Bundled ${destination} (sha256: ${sidecars[sidecar]})`)
    }
    if (goos === 'windows') {
      const hashes = join(
        project,
        'src-tauri',
        'packages',
        'windows',
        'core-hashes.nsh',
      )
      writeFileSync(
        hashes,
        `!define MIHOMO_SHA256 "${sidecars[`verge-mihomo-${target}.exe`]}"\n!define MIHOMO_ALPHA_SHA256 "${sidecars[`verge-mihomo-alpha-${target}.exe`]}"\n`,
      )
      console.log(`Generated ${hashes}`)
    }
  }
  builds[target] = { artifact: name, goos, goarch, nativeVersion, sidecars }
}
const metadata = {
  upstream,
  tag,
  commit,
  version,
  toolchain,
  buildTime,
  tags,
  goamd64: 'v2',
  cgo: false,
  patches,
  artifacts,
  builds,
}
writeFileSync(
  join(output, 'build.json'),
  `${JSON.stringify(metadata, null, 2)}\n`,
)
console.log(`Custom core output: ${output}`)
console.log(
  values.bundle
    ? 'Both core IDs bundle identical custom bytes; no installed app was changed.'
    : 'Bundling requires --bundle; no installed app or sidecar was changed.',
)
