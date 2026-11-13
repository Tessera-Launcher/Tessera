import { readdir, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import Color from 'colorjs.io';
import { parseTokens } from './lib/token-lib.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const contrast = (a, b) => Color.contrast(new Color('srgb', a), new Color('srgb', b), 'WCAG21');
const page = ['background.canvas', 'background.surface', 'background.raised'];
const surfaces = [...page, 'background.hover', 'background.selected'];
const colorLiteral = /(?:^|[^\w])#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})\b|\b0x[0-9a-fA-F]{6,8}\b|\brgba?\(\s*\d/;

export function paletteFields(rust) {
  return new Set([...rust.matchAll(/pub (\w+): u32,/g)].map(match => match[1]));
}

export function checkRustFields(source, fields) {
  return [...source.matchAll(/(?<!theme\.)\bcolors\.(\w+)/g)]
    .filter(match => !fields.has(match[1]))
    .map(match => match[1]);
}

const RUST_ROLE_METHODS = {
  space: new Set(['p', 'px', 'py', 'pt', 'pb', 'pl', 'pr', 'm', 'mx', 'my', 'mt', 'mb', 'ml', 'mr', 'gap', 'row_gap', 'column_gap']),
  layout: new Set(['w', 'h', 'min_w', 'min_h', 'max_w', 'max_h', 'size', 'width', 'height']),
  radius: new Set(['rounded', 'rounded_t', 'rounded_b', 'rounded_l', 'rounded_r', 'rounded_tl', 'rounded_tr', 'rounded_bl', 'rounded_br']),
  stroke: new Set(['border', 'border_t', 'border_b', 'border_l', 'border_r']),
  font: new Set(['text_size', 'line_height']),
  weight: new Set(['font_weight']),
  motion: new Set(['from_millis']),
};

export function checkRustRoleUsage(source) {
  const errors = [];
  const lines = source.split('\n');
  for (let index = 0; index < lines.length; index++) {
    for (const match of lines[index].matchAll(/\.([a-z_]+)\([^)]*?\b(space|layout|radius|stroke|font|weight|motion)::/g)) {
      const [, method, role] = match;
      if (!RUST_ROLE_METHODS[role]?.has(method)) errors.push(`${index + 1}: ${method} uses ${role}`);
    }
  }
  return errors;
}

const CSS_ROLES = {
  // `text-on-fill` is the paint for marks/icons placed on a fill; a logo mark
  // may use it as a shape background or border.
  // Backgrounds may paint separators/indicators in border tones.
  background: ['background-', 'fill-', 'border-', 'text-on-fill'],
  'background-color': ['background-', 'fill-', 'border-', 'text-on-fill'],
  color: ['text-', 'status-'],
  border: ['border-', 'fill-', 'text-on-fill'],
  'border-color': ['border-', 'fill-', 'text-on-fill'],
  outline: ['border-'],
  'outline-color': ['border-'],
  'box-shadow': ['border-', 'background-'],
  'accent-color': ['fill-', 'text-'],
  'scrollbar-color': ['border-', 'background-', 'fill-'],
};

/** Returns color literals that bypass the token system. */
export function findLiteralColors(text) {
  return text.split('\n').flatMap((line, index) => (colorLiteral.test(line) ? [index + 1] : []));
}

/** Checks that each color token is used by a property matching its role. */
export function checkCssRoles(css) {
  const errors = [];
  const declaration = /([a-z-]+)\s*:\s*([^;{}]+);/g;
  for (const match of css.matchAll(declaration)) {
    const property = match[1];
    const allowed = CSS_ROLES[property];
    if (!allowed) continue;
    for (const token of match[2].matchAll(/var\(--color-([a-z-]+)\)/g)) {
      const role = token[1];
      if (!allowed.some(prefix => role.startsWith(prefix))) {
        errors.push(`${property} uses --color-${role}`);
      }
    }
  }
  return errors;
}

async function walk(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) files.push(...await walk(path));
    else files.push(path);
  }
  return files;
}

async function loadRoles() {
  const resolverURL = new URL('design/tokens/tessera.resolver.json', new URL('../../', import.meta.url));
  const descriptor = JSON.parse(await readFile(resolverURL, 'utf8'));
  const { resolver } = await parseTokens({ filename: resolverURL, src: descriptor });
  return Object.fromEntries(Object.keys(descriptor.modifiers.colorScheme.contexts).map(mode => [mode, resolver.apply({ colorScheme: mode })]));
}

export function checkDescriptions(tokens) {
  return ['text.primary', 'text.secondary', 'text.accent', 'text.accentHover', 'text.onFill', 'status.success', 'status.warning', 'status.danger', 'fill.primary', 'fill.primaryHover', 'fill.neutral', 'fill.warning', 'fill.danger', 'border.strong', 'border.focus']
    .filter(path => !tokens[path]?.$description)
    .map(path => `${path} has no description`);
}

export function checkSemantics(tokens) {
  const errors = [];
  const rgb = path => tokens[path]?.$value?.components;
  const require = path => {
    if (!rgb(path)) errors.push(`missing role ${path}`);
  };
  for (const path of ['text.primary', 'text.secondary', 'text.accent', 'text.accentHover', 'text.onFill', 'status.success', 'status.warning', 'status.danger', 'fill.primary', 'fill.primaryHover', 'fill.neutral', 'fill.warning', 'fill.danger', 'border.strong', 'border.focus']) require(path);
  if (errors.length) return errors;
  for (const fg of ['text.primary', 'text.secondary', 'text.accent', 'text.accentHover', 'status.success', 'status.warning', 'status.danger']) {
    for (const bg of surfaces) {
      const ratio = contrast(rgb(fg), rgb(bg));
      if (ratio < 4.5) errors.push(`${fg} on ${bg} is ${ratio.toFixed(2)}:1`);
    }
  }
  for (const fill of ['fill.primary', 'fill.primaryHover']) {
    const ratio = contrast(rgb('text.onFill'), rgb(fill));
    if (ratio < 4.5) errors.push(`text.onFill on ${fill} is ${ratio.toFixed(2)}:1`);
    for (const bg of page.slice(0, 2)) {
      const boundary = contrast(rgb(fill), rgb(bg));
      if (boundary < 3) errors.push(`${fill} on ${bg} is ${boundary.toFixed(2)}:1`);
    }
  }
  for (const fill of ['fill.neutral', 'fill.warning', 'fill.danger']) {
    for (const bg of page) {
      const ratio = contrast(rgb(fill), rgb(bg));
      if (ratio < 3) errors.push(`${fill} on ${bg} is ${ratio.toFixed(2)}:1`);
    }
  }
  for (const border of ['border.strong', 'border.focus']) {
    for (const bg of page) {
      const ratio = contrast(rgb(border), rgb(bg));
      if (ratio < 3) errors.push(`${border} on ${bg} is ${ratio.toFixed(2)}:1`);
    }
  }
  return errors;
}

async function main() {
  const errors = [];
  const mode = process.argv.includes('--contrast') ? 'contrast'
    : process.argv.includes('--descriptions') ? 'descriptions'
    : process.argv.includes('--usage') ? 'usage'
    : 'all';
  const modes = await loadRoles();
  if (JSON.stringify(Object.keys(modes.dark).sort()) !== JSON.stringify(Object.keys(modes.light).sort())) {
    errors.push('light and dark roles differ');
  }
  if (mode === 'contrast' || mode === 'all') {
    for (const [scheme, tokens] of Object.entries(modes)) {
      for (const error of checkSemantics(tokens)) errors.push(`${scheme}: ${error}`);
    }
  }
  if (mode === 'contrast') {
    if (errors.length) {
      for (const error of errors) console.error(error);
      process.exitCode = 1;
      return;
    }
    console.log('Contrast checks passed for 2 schemes.');
    return;
  }
  if (mode === 'descriptions' || mode === 'all') {
    for (const [scheme, tokens] of Object.entries(modes)) {
      for (const error of checkDescriptions(tokens)) errors.push(`${scheme}: ${error}`);
    }
  }
  if (mode === 'descriptions') {
    if (errors.length) {
      for (const error of errors) console.error(error);
      process.exitCode = 1;
      return;
    }
    console.log('Description checks passed for 2 schemes.');
    return;
  }

  const generated = await readFile(resolve(root, 'crates/tessera-gui/src/design_tokens.rs'), 'utf8');
  const fields = paletteFields(generated);
  const usedFields = new Set();
  const rustFiles = (await walk(resolve(root, 'crates/tessera-gui/src')))
    .filter(file => file.endsWith('.rs') && !file.endsWith('design_tokens.rs'));
  for (const file of rustFiles) {
    const source = await readFile(file, 'utf8');
    for (const line of findLiteralColors(source)) errors.push(`${file.slice(root.length + 1)}:${line} has a literal color`);
    for (const error of checkRustRoleUsage(source)) errors.push(`${file.slice(root.length + 1)}:${error}`);
    for (const field of checkRustFields(source, fields)) errors.push(`${file.slice(root.length + 1)} references unknown Palette.${field}`);
    for (const match of source.matchAll(/(?<!theme\.)\bcolors\.(\w+)/g)) usedFields.add(match[1]);
  }
  for (const field of fields) {
    if (!usedFields.has(field)) errors.push(`Palette.${field} is never used`);
  }

  const css = await readFile(resolve(root, 'preview/styles.css'), 'utf8');
  for (const line of findLiteralColors(css)) errors.push(`preview/styles.css:${line} has a literal color`);
  for (const error of checkCssRoles(css)) errors.push(`preview/styles.css: ${error}`);

  if (errors.length) {
    for (const error of errors) console.error(error);
    process.exitCode = 1;
    return;
  }
  console.log(`Usage checks passed for ${Object.keys(modes).length} schemes: literals, property compatibility, palette fields.`);
}

if (process.argv[1]?.replaceAll('\\', '/').endsWith('/design/scripts/check-usage.mjs')) main().catch(error => {
  console.error(error.message);
  process.exitCode = 1;
});
