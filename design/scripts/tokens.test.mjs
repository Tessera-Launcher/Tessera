import { test } from 'node:test';
import assert from 'node:assert/strict';
import Color from 'colorjs.io';
import { readFile } from 'node:fs/promises';
import { parseTokens, renderTokens } from './lib/token-lib.mjs';
import { generateDesign } from '../generation/src/color.mjs';
import { validateConfig } from '../generation/src/config.mjs';
import { validateRecipe } from '../generation/src/recipe.mjs';
import { validateDocument } from './validate-tokens.mjs';
import { checkCssRoles, checkRustFields, checkRustRoleUsage, findLiteralColors } from './check-usage.mjs';
import { borderWidthPx, dimensionPx, fontSizePx, lineHeightPx, radiusPx } from '../generation/src/scales.mjs';

const read = path => readFile(new URL(path, import.meta.url), 'utf8').then(JSON.parse);
const recipe = await read('../generation/recipes/color.json');
const config = await read('../generation/config.json');
const buildArtifacts = async document => renderTokens((await parseTokens({ src: document })).tokens);
const designArtifacts = async (design, mode) => buildArtifacts({ ...design.primitive, ...design.themes[mode] });
const luminance = rgb => rgb.map(c => c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4).reduce((sum, c, i) => sum + c * [.2126, .7152, .0722][i], 0);
const contrast = (a, b) => { const values = [luminance(a), luminance(b)].sort((a, b) => b - a); return (values[0] + .05) / (values[1] + .05); };
const brandKey = changes => {
  const changed = structuredClone(recipe);
  const brand = changed.colors.find(color => color.name === 'brand');
  brand.key.components = changes;
  delete brand.key.hex;
  return changed;
};

// Cross-language output is our contract; DTCG parsing belongs to Terrazzo.
test('resolved aliases, alpha and generic font fallbacks agree in Rust and CSS', async () => {
  const fixture = {
    font: { family: { $type: 'fontFamily', $value: ['Example Font', 'sans-serif'] } },
    palette: { $type: 'color', ink: { $value: { colorSpace: 'srgb', components: [1, 0.5, 0], alpha: 0.5 } } },
    color: { $type: 'color', base: { $value: '{palette.ink}' }, foreground: { $value: '{color.base}' } },
    space: { $type: 'dimension', inset: { $value: { value: 12, unit: 'px' } } },
  };
  const { css, rust } = await buildArtifacts(fixture);
  assert.match(css, /--color-foreground: #ff800080;/);
  assert.match(rust, /pub const FOREGROUND: u32 = 0xff800080;/);
  assert.match(css, /--font-family: "Example Font", sans-serif;/);
  assert.match(css, /--space-inset: 12px;/);
  assert.match(rust, /pub const INSET: f32 = 12\.0;/);
});

test('invalid tokens, references and unsupported GPUI units fail the build', async () => {
  for (const fixture of [
    { color: { $type: 'color', a: { $value: '{color.b}' }, b: { $value: '{color.a}' } } },
    { color: { $type: 'color', a: { $value: '{color.missing}' } } },
    { color: { a: { $type: 'color', $value: '{number.a}' } }, number: { a: { $type: 'number', $value: 1 } } },
    { space: { a: { $type: 'dimension', $value: { value: 1, unit: 'rem' } } } },
    // Detect accidentally bypassing Terrazzo's value validation in the JS API.
    { font: { weight: { $type: 'fontWeight', $value: 2000 } } },
  ]) await assert.rejects(buildArtifacts(fixture));
});

// Terrazzo's lint accepts a numeric $description; the official schema must not.
test('the vendored DTCG schema rejects non-standard documents Terrazzo parses', async () => {
  assert.deepEqual(await validateDocument('format', await read('../tokens/foundation/color.tokens.json')), []);
  const resolver = await read('../tokens/tessera.resolver.json');
  assert.deepEqual(await validateDocument('resolver', resolver), []);
  const nonStandard = { space: { sample: { $type: 'dimension', $value: { value: 8, unit: 'px' }, $description: 123 } } };
  await buildArtifacts(nonStandard);
  assert.notDeepEqual(await validateDocument('format', nonStandard), []);
});

test('generation config and color recipe reject values the generator cannot use', () => {
  assert.doesNotThrow(() => validateConfig(config));
  const invalidTone = structuredClone(config);
  invalidTone.color.lightness.dark.bg = 1.4;
  assert.throws(() => validateConfig(invalidTone), /lightness\/dark\/bg/);
  const unknownRole = structuredClone(config);
  unknownRole.color.lightness.light.unknown = 0.5;
  assert.throws(() => validateConfig(unknownRole), /additional properties/);

  assert.doesNotThrow(() => validateRecipe(recipe));
  const inverted = structuredClone(recipe);
  inverted.colors[0].saturation = { min: 0.8, max: 0.2 };
  assert.throws(() => validateRecipe(inverted), /saturation min/);
});

test('generated palettes meet text, fill and boundary contrast after sRGB quantization', async () => {
  // Include saturated alternatives so this tests the algorithm, not just one tuned palette.
  for (const brand of [null, [1, 0, 0], [.45, .1, .85], [1, .45, 0]]) {
    const design = generateDesign({ recipe: brand ? brandKey(brand) : recipe, config });
    for (const mode of Object.keys(design.themes)) {
      const { tokens } = await designArtifacts(design, mode);
      const rgb = path => tokens.get(path).value.components;
      const surfaces = ['background.canvas', 'background.surface', 'background.raised', 'background.hover', 'background.selected'];
      const page = surfaces.slice(0, 3);
      for (const fg of ['text.primary', 'text.secondary', 'text.accent', 'text.accentHover', 'status.success', 'status.warning', 'status.danger']) {
        for (const bg of surfaces) assert.ok(contrast(rgb(fg), rgb(bg)) >= 4.5, `${mode}: ${fg} on ${bg}`);
      }
      for (const fill of ['fill.primary', 'fill.primaryHover']) {
        assert.ok(contrast(rgb('text.onFill'), rgb(fill)) >= 4.5, `${mode}: text.onFill on ${fill}`);
        for (const bg of page.slice(0, 2)) assert.ok(contrast(rgb(fill), rgb(bg)) >= 3, `${mode}: ${fill} on ${bg}`);
      }
      for (const fill of ['fill.warning', 'fill.danger', 'fill.neutral']) {
        for (const bg of page) assert.ok(contrast(rgb(fill), rgb(bg)) >= 3, `${mode}: ${fill} on ${bg}`);
      }
      for (const border of ['border.strong', 'border.focus']) {
        for (const bg of page) assert.ok(contrast(rgb(border), rgb(bg)) >= 3, `${mode}: ${border} on ${bg}`);
      }
    }
  }
});

test('the palette keeps one brand hue and neutral surfaces', async () => {
  const design = generateDesign({ recipe, config });
  for (const mode of Object.keys(design.themes)) {
    const { tokens } = await designArtifacts(design, mode);
    const oklch = path => new Color('srgb', tokens.get(path).value.components).to('oklch').coords;
    const [,, brandHue] = oklch('text.accent');
    const hueDistance = hue => Math.min(Math.abs(hue - brandHue), 360 - Math.abs(hue - brandHue));
    for (const role of ['fill.primary', 'background.selected']) {
      const [lightness, chroma, hue] = oklch(role);
      assert.ok(hueDistance(hue) <= 15, `${mode}: ${role} hue ${hue} drifted from ${brandHue}`);
      // Large selection fills carry less chroma than small accents; both must stay chromatic.
      assert.ok(chroma >= (role === 'background.selected' ? 0.01 : 0.03), `${mode}: ${role} chroma ${chroma} is too gray`);
      assert.ok(lightness > 0 && lightness < 1, `${mode}: ${role} is not a color`);
    }
    for (const role of ['background.canvas', 'background.surface', 'background.raised', 'background.hover']) {
      const [, chroma] = oklch(role);
      assert.ok(chroma <= 0.02, `${mode}: ${role} gained an unrelated tint (${chroma})`);
    }
  }
});

test('a key color change propagates while neutral surfaces stay independent', async () => {
  const original = generateDesign({ recipe, config });
  const alternate = generateDesign({ recipe: brandKey([.8, .1, .2]), config });
  for (const mode of ['light', 'dark']) {
    const before = (await designArtifacts(original, mode)).tokens;
    const after = (await designArtifacts(alternate, mode)).tokens;
    for (const role of ['fill.primary', 'text.accent', 'background.selected']) {
      assert.notDeepEqual(before.get(role).value, after.get(role).value, `${mode}: ${role}`);
    }
    for (const role of ['background.canvas', 'background.surface', 'background.raised']) {
      assert.deepEqual(before.get(role).value, after.get(role).value, `${mode}: ${role}`);
    }
    assert.deepEqual(after.get('status.success').value, after.get('text.accent').value);
  }
});

test('scale models derive the shipped sizes, radii, borders and line heights', () => {
  assert.deepEqual([50, 200, 1200, 5200].map(step => dimensionPx(step)), [2, 8, 48, 208]);
  assert.deepEqual([200, 300, 600, 900].map(step => fontSizePx(step)), [12, 14, 21, 32]);
  assert.deepEqual([12, 14, 21, 32].map(size => lineHeightPx(size)), [16, 20, 28, 40]);
  assert.deepEqual([100, 200, 300].map(step => radiusPx(step)), [3, 6, 12]);
  assert.deepEqual([100, 200].map(step => borderWidthPx(step)), [1, 2]);
});

test('usage checks reject literals, mismatched roles and unknown palette fields', () => {
  assert.deepEqual(findLiteralColors('const color = rgb(0x112233);'), [1]);
  assert.deepEqual(findLiteralColors('const color = rgb(colors.fill_primary);'), []);
  assert.deepEqual(checkCssRoles('.a { color: var(--color-fill-primary); }'), ['color uses --color-fill-primary']);
  assert.deepEqual(checkCssRoles('.a { color: var(--color-text-primary); background: var(--color-fill-primary); }'), []);
  assert.deepEqual(checkRustFields('colors.fill_primary; colors.accent;', new Set(['fill_primary'])), ['accent']);
  assert.deepEqual(checkRustRoleUsage('.gap(px(space::MD))'), []);
  assert.deepEqual(checkRustRoleUsage('.gap(px(layout::SIDEBAR))'), ['1: gap uses layout']);
});

test('the shipped light and dark themes contain no green hues', async () => {
  const design = generateDesign({ recipe, config });
  for (const mode of Object.keys(design.themes)) {
    const { tokens } = await designArtifacts(design, mode);
    for (const [path, token] of tokens) {
      const [r, g, b] = token.value.components;
      const max = Math.max(r, g, b), min = Math.min(r, g, b), delta = max - min;
      if (delta < .04) continue;
      const hue = ((max === r ? (g - b) / delta : max === g ? (b - r) / delta + 2 : (r - g) / delta + 4) * 60 + 360) % 360;
      assert.ok(hue < 95 || hue > 170, `${mode}/${path} introduces a green hue (${hue}°)`);
    }
  }
});
