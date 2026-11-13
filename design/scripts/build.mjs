import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseTokens, renderTokens } from './lib/token-lib.mjs';

const rootURL = new URL('../../', import.meta.url);
const root = fileURLToPath(rootURL);
const resolverURL = new URL('design/tokens/tessera.resolver.json', rootURL);
const descriptor = JSON.parse(await readFile(resolverURL, 'utf8'));
// Read the committed sources; the check chain runs generate --check before this.
const { resolver } = await parseTokens({ filename: resolverURL, src: descriptor });
const modes = Object.keys(descriptor.modifiers.colorScheme.contexts);
const resolved = Object.fromEntries(modes.map(mode => [mode, resolver.apply({ colorScheme: mode })]));

const semanticGroups = new Set(['background', 'text', 'fill', 'border', 'status']);
const isPrimitive = id => id.startsWith('primitive.');
const isSemantic = id => semanticGroups.has(id.split('.')[0]);
const rolePaths = Object.keys(resolved.dark).filter(isSemantic).sort();
for (const mode of modes) {
  const paths = Object.keys(resolved[mode]).filter(isSemantic).sort();
  if (paths.join('\n') !== rolePaths.join('\n')) throw new Error(`Light and dark roles differ in ${mode}`);
}

const common = renderTokens(Object.fromEntries(
  Object.entries(resolved.dark).filter(([id]) => !isPrimitive(id) && !isSemantic(id)),
));
const palettes = Object.fromEntries(Object.entries(resolved).map(([mode, tokens]) => [mode, renderTokens(
  Object.fromEntries(Object.entries(tokens).filter(([id]) => isSemantic(id))),
  { cssPrefix: 'color-' },
)]));

const fields = [
  ['canvas', 'background.canvas'],
  ['surface', 'background.surface'],
  ['raised', 'background.raised'],
  ['hover', 'background.hover'],
  ['selected', 'background.selected'],
  ['backdrop', 'background.backdrop'],
  ['text_primary', 'text.primary'],
  ['text_secondary', 'text.secondary'],
  ['text_accent', 'text.accent'],
  ['text_accent_hover', 'text.accentHover'],
  ['text_on_fill', 'text.onFill'],
  ['fill_primary', 'fill.primary'],
  ['fill_primary_hover', 'fill.primaryHover'],
  ['fill_neutral', 'fill.neutral'],
  ['fill_warning', 'fill.warning'],
  ['fill_danger', 'fill.danger'],
  ['border_default', 'border.default'],
  ['border_strong', 'border.strong'],
  ['border_focus', 'border.focus'],
  ['status_success', 'status.success'],
  ['status_warning', 'status.warning'],
  ['status_danger', 'status.danger'],
];
for (const [field, path] of fields) {
  if (!rolePaths.includes(path)) throw new Error(`Missing role ${path} for Palette.${field}`);
}

const rust = [
  common.rust,
  'pub mod color {',
  '    #[derive(Clone, Copy)]',
  '    pub struct Palette {',
  ...fields.map(([field]) => `        pub ${field}: u32,`),
  '    }',
];
for (const mode of modes) {
  const rendered = palettes[mode].values;
  rust.push(`    pub const ${mode.toUpperCase()}: Palette = Palette {`);
  for (const [field, path] of fields) {
    const value = rendered.get(path);
    if (!value) throw new Error(`Missing rendered role ${path}`);
    rust.push(`        ${field}: ${value.rust},`);
  }
  rust.push('    };');
}
rust.push('}', '');

const baseMode = descriptor.modifiers.colorScheme.default;
const alternateMode = modes.find(mode => mode !== baseMode);
const css = common.css + '\n'
  + palettes[baseMode].css.replace(':root {', `:root, :root[data-theme="${baseMode}"] {\n  color-scheme: ${baseMode};`) + '\n'
  + palettes[alternateMode].css.replace(':root {', `:root[data-theme="${alternateMode}"] {\n  color-scheme: ${alternateMode};`);

const outputs = new Map([
  ['preview/tokens.css', css],
  ['crates/tessera-gui/src/design_tokens.rs', rust.join('\n')],
]);
const check = process.argv.includes('--check');
for (const [path, content] of outputs) {
  const target = resolve(root, path);
  if (check) {
    if (await readFile(target, 'utf8') !== content) throw new Error(`${path} is stale. Run npm run build.`);
  } else {
    await mkdir(dirname(target), { recursive: true });
    await writeFile(target, content);
  }
}
console.log(`${check ? 'Checked' : 'Built'} ${fields.length} semantic roles × ${modes.length} schemes + shared tokens for Rust and CSS.`);
