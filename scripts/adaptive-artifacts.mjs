import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, join, resolve } from 'node:path'

const [coreOutput, artifactOutput] = process.argv
  .slice(2, 4)
  .map((path) => resolve(path))
const appOnly = process.argv[4] === '--app'
const target = process.env.ADAPTIVE_TARGET
if (!coreOutput || !artifactOutput || !target)
  throw new Error('Expected core output, artifact output and ADAPTIVE_TARGET')
const windows = target.includes('windows')
const bundle = resolve('target', target, 'release', 'bundle')
const metadata = JSON.parse(
  readFileSync(join(coreOutput, 'build.json'), 'utf8'),
)
const digest = (path) =>
  createHash('sha256').update(readFileSync(path)).digest('hex')
function run(command, args) {
  const result = spawnSync(command, args, { encoding: 'utf8' })
  if (result.error) throw result.error
  if (result.status !== 0)
    throw new Error(`${command}: ${result.stdout}\n${result.stderr}`)
  return result.stdout
}
function findFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name)
    return entry.isDirectory() ? findFiles(path) : [path]
  })
}
const extension = windows ? '.exe' : ''
let coreDirectory
let installers
if (windows) {
  installers = findFiles(join(bundle, 'nsis')).filter((path) =>
    path.endsWith('-setup.exe'),
  )
  if (installers.length !== 1) throw new Error('Expected one NSIS installer')
  const extracted = mkdtempSync(join(tmpdir(), 'adaptive-installer-'))
  run('7z', ['x', '-y', `-o${extracted}`, installers[0]])
  const cores = findFiles(extracted).filter(
    (path) => basename(path) === 'verge-mihomo.exe',
  )
  if (cores.length !== 1) throw new Error('Expected one packaged stable core')
  coreDirectory = resolve(cores[0], '..')
} else if (appOnly) {
  installers = []
  coreDirectory = join(bundle, 'macos', 'Clash Verge.app', 'Contents', 'MacOS')
} else {
  installers = findFiles(join(bundle, 'dmg')).filter((path) =>
    path.endsWith('.dmg'),
  )
  if (installers.length !== 1) throw new Error('Expected one DMG')
  const mount = mkdtempSync(join(tmpdir(), 'adaptive-dmg-'))
  run('hdiutil', [
    'attach',
    '-readonly',
    '-nobrowse',
    '-mountpoint',
    mount,
    installers[0],
  ])
  process.on('exit', () => spawnSync('hdiutil', ['detach', mount]))
  coreDirectory = join(mount, 'Clash Verge.app', 'Contents', 'MacOS')
}
if (!windows) {
  const app = resolve(coreDirectory, '..', '..')
  run('codesign', ['--verify', '--deep', '--strict', '--verbose=2', app])
  const arch = target.startsWith('aarch64') ? 'arm64' : 'x86_64'
  const mainBinary = run('/usr/libexec/PlistBuddy', [
    '-c',
    'Print :CFBundleExecutable',
    join(app, 'Contents', 'Info.plist'),
  ]).trim()
  for (const name of [mainBinary, 'verge-mihomo', 'verge-mihomo-alpha']) {
    run('lipo', [join(coreDirectory, name), '-verify_arch', arch])
  }
}
const stable = join(coreDirectory, `verge-mihomo${extension}`)
const compatibility = join(coreDirectory, `verge-mihomo-alpha${extension}`)
const coreHash = digest(stable)
if (coreHash !== digest(compatibility))
  throw new Error('Packaged core entries differ')
const builtCores = Object.keys(metadata.artifacts)
if (builtCores.length !== 1)
  throw new Error('Expected one target-specific core')
const original = join(coreOutput, builtCores[0])
if (windows) {
  if (coreHash !== digest(original))
    throw new Error('Installer changed the custom core')
} else {
  // Tauri may replace Go's ad-hoc signature; compare the executable payload independently.
  const comparison = mkdtempSync(join(tmpdir(), 'adaptive-signatures-'))
  const copies = [original, stable].map((path, index) => {
    const copy = join(comparison, `core-${index}`)
    copyFileSync(path, copy)
    run('codesign', ['--remove-signature', copy])
    return copy
  })
  if (digest(copies[0]) !== digest(copies[1]))
    throw new Error('Packaging changed the custom core payload')
}
const version = run(stable, ['-v']).trim()
if (!version.split(/\s+/).includes(metadata.version))
  throw new Error(`Wrong packaged version: ${version}`)
mkdirSync(artifactOutput, { recursive: true })
if (appOnly) {
  const archive = join(
    artifactOutput,
    'Clash.Verge_2.5.9_arm64_CloudflarePersistentBindings.app.zip',
  )
  run('ditto', [
    '-c',
    '-k',
    '--sequesterRsrc',
    '--keepParent',
    resolve(coreDirectory, '..', '..'),
    archive,
  ])
  installers.push(archive)
}
for (const installer of installers) {
  const destination = join(artifactOutput, basename(installer))
  if (installer !== destination) copyFileSync(installer, destination)
}
copyFileSync(
  join(coreOutput, 'build.json'),
  join(artifactOutput, 'core-build.json'),
)
copyFileSync('LICENSE', join(artifactOutput, 'LICENSE'))
writeFileSync(
  join(artifactOutput, 'package.json'),
  `${JSON.stringify(
    {
      target,
      source: `https://github.com/${process.env.GITHUB_REPOSITORY}/tree/${process.env.GITHUB_SHA}`,
      version,
      packagedCoreSha256: coreHash,
      identicalCoreEntries: true,
      appleNotarized: false,
      installers: Object.fromEntries(
        installers.map((path) => [basename(path), digest(path)]),
      ),
    },
    null,
    2,
  )}\n`,
)
const hashes = readdirSync(artifactOutput).map(
  (name) => `${digest(join(artifactOutput, name))}  ${name}`,
)
writeFileSync(join(artifactOutput, 'SHA256SUMS'), `${hashes.join('\n')}\n`)
console.log(`Verified identical custom core entries: ${coreHash}`)
