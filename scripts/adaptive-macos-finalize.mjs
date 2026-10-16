import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { copyFileSync, mkdtempSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const [coreOutput, app] = process.argv.slice(2).map((path) => resolve(path))
if (process.platform !== 'darwin' || !coreOutput || !app)
  throw new Error('Expected core output and application paths on macOS')
function run(command, args) {
  const result = spawnSync(command, args, { encoding: 'utf8' })
  if (result.error) throw result.error
  if (result.status !== 0)
    throw new Error(`${command}: ${result.stdout}\n${result.stderr}`)
}
const digest = (path) =>
  createHash('sha256').update(readFileSync(path)).digest('hex')
const metadata = JSON.parse(
  readFileSync(join(coreOutput, 'build.json'), 'utf8'),
)
const artifacts = Object.keys(metadata.artifacts)
if (artifacts.length !== 1) throw new Error('Expected one target-specific core')
const original = join(coreOutput, artifacts[0])
if (digest(original) !== metadata.artifacts[artifacts[0]])
  throw new Error('Original core hash differs from build metadata')
const stable = join(app, 'Contents', 'MacOS', 'verge-mihomo')
const compatibility = join(app, 'Contents', 'MacOS', 'verge-mihomo-alpha')
const comparison = mkdtempSync(join(tmpdir(), 'adaptive-finalize-'))
const payloadHashes = [original, stable, compatibility].map((path, index) => {
  const copy = join(comparison, `core-${index}`)
  copyFileSync(path, copy)
  run('codesign', ['--remove-signature', copy])
  return digest(copy)
})
if (new Set(payloadHashes).size !== 1)
  throw new Error('Packaged core payload differs from the original custom core')

// Re-sign one shared executable, not two filename-derived signing identities.
run('codesign', [
  '--force',
  '--sign',
  '-',
  '--identifier',
  'io.github.clash-verge-rev.mihomo',
  '--options',
  'runtime',
  '--entitlements',
  'src-tauri/packages/macos/entitlements.plist',
  stable,
])
copyFileSync(stable, compatibility)
run('codesign', [
  '--force',
  '--sign',
  '-',
  '--options',
  'runtime',
  '--entitlements',
  'src-tauri/packages/macos/entitlements.plist',
  app,
])
run('codesign', ['--verify', '--deep', '--strict', '--verbose=2', app])
if (digest(stable) !== digest(compatibility))
  throw new Error('Final signed core entries differ')
console.log(
  `Verified original payload and identical signed core entries: ${digest(stable)}`,
)
