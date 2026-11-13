import { defineConfig, parse } from '@terrazzo/parser';

// The JS API needs an initialized config to run Terrazzo's recommended validation.
const config = defineConfig({}, { cwd: new URL('../../../', import.meta.url) });
// Keep existing semantic names (onAccent, borderStrong, etc.).
config.lint.rules['core/consistent-naming'] = ['off', {}];

export function parseTokens(document, options = {}) {
  return parse(document, { ...options, config });
}

// This adapter only serializes the types used by GPUI and our CSS preview.
export function renderTokens(normalized, { cssPrefix = '' } = {}) {
  const tokens = new Map(Object.entries(normalized).map(([path, token]) => [path, {
    type: token.$type, value: token.$value, parts: path.split('.'),
  }]));
  const resolve = path => tokens.get(path);
  const values = new Map();
  const kebab = value => value.replace(/([a-z0-9])([A-Z])/g, '$1-$2').toLowerCase();
  const rustName = value => kebab(value).replaceAll('-', '_').toUpperCase();
  const float = value => Number.isInteger(value) ? `${value}.0` : String(value);
  const modules = new Map();
  const css = ['/* Generated from design/tokens. Run npm run build. */', ':root {'];
  for (const path of tokens.keys()) {
    const { type, value, parts } = resolve(path);
    let cssValue, rustType, rustValue;
    switch (type) {
      case 'color': {
        if (value.colorSpace !== 'srgb' || value.components.length !== 3 || value.components.some(x => typeof x !== 'number' || x < 0 || x > 1)) throw new Error(`Only numeric sRGB colors in [0,1] are supported: ${path}`);
        const alpha = value.alpha ?? 1;
        if (typeof alpha !== 'number' || alpha < 0 || alpha > 1) throw new Error(`Invalid alpha: ${path}`);
        const channels = [...value.components, ...(alpha < 1 ? [alpha] : [])];
        const hex = channels.map(x => Math.round(x * 255).toString(16).padStart(2, '0')).join('');
        cssValue = `#${hex}`;
        rustType = 'u32';
        rustValue = `0x${hex}`;
        break;
      }
      case 'dimension':
        if (value.unit !== 'px' || !Number.isFinite(value.value)) throw new Error(`Only pixel dimensions are supported: ${path}`);
        cssValue = `${value.value}px`;
        rustType = 'f32'; rustValue = float(value.value); break;
      case 'number':
      case 'fontWeight':
        if (!Number.isFinite(value)) throw new Error(`Expected a number: ${path}`);
        cssValue = String(value); rustType = 'f32'; rustValue = float(value); break;
      case 'fontFamily': {
        const families = Array.isArray(value) ? value : [value];
        if (!families.length || families.some(x => typeof x !== 'string' || !/^[\w .-]+$/.test(x))) throw new Error(`Invalid font family: ${path}`);
        const generic = new Set(['serif', 'sans-serif', 'monospace', 'system-ui', 'cursive', 'fantasy', 'ui-serif', 'ui-sans-serif', 'ui-monospace', 'ui-rounded', 'emoji', 'math', 'fangsong']);
        cssValue = families.map(x => generic.has(x) ? x : JSON.stringify(x)).join(', ');
        rustType = `[&str; ${families.length}]`;
        rustValue = `[${families.map(x => JSON.stringify(x)).join(', ')}]`; break;
      }
      case 'duration':
        if (!['ms', 's'].includes(value.unit) || !Number.isFinite(value.value) || value.value < 0) throw new Error(`Invalid duration: ${path}`);
        cssValue = `${value.value}${value.unit}`; rustType = 'u64'; rustValue = String(Math.round(value.value * (value.unit === 's' ? 1000 : 1))); break;
      case 'cubicBezier':
        if (!Array.isArray(value) || value.length !== 4 || value.some(x => !Number.isFinite(x)) || [value[0], value[2]].some(x => x < 0 || x > 1)) throw new Error(`Invalid easing: ${path}`);
        cssValue = `cubic-bezier(${value.join(', ')})`; rustType = '[f32; 4]'; rustValue = `[${value.map(float).join(', ')}]`; break;
      default: throw new Error(`Unsupported token type ${type}: ${path}`);
    }
    values.set(path, { css: cssValue, rust: rustValue, rustType });
    css.push(`  --${cssPrefix}${parts.map(kebab).join('-')}: ${cssValue};`);
    const group = parts[0].toLowerCase();
    if (parts.length < 2) throw new Error(`Tokens must be grouped: ${path}`);
    const name = parts.slice(1).map(rustName).join('_');
    const declarations = modules.get(group) ?? new Map();
    if (declarations.has(name)) throw new Error(`Generated name collision: ${path}`);
    declarations.set(name, `    pub const ${name}: ${rustType} = ${rustValue};`);
    modules.set(group, declarations);
  }
  css.push('}', '');
  const rust = ['// Generated from design/tokens. Run npm run build.', '// Colors with alpha are RGBA (use rgba); opaque colors are RGB (use rgb).', '#![allow(dead_code)]', ''];
  for (const [group, declarations] of modules) rust.push(`pub mod ${group} {`, ...declarations.values(), '}', '');
  return { css: css.join('\n'), rust: rust.join('\n'), tokens, values };
}
