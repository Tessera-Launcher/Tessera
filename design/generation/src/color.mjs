import Color from 'colorjs.io';
import { keyHue, lightnessOf, rampColor, rampSteps } from './ramp.mjs';

const contrast = (a, b) => Color.contrast(new Color('srgb', a), new Color('srgb', b), 'WCAG21');
const surfaceRoles = ['bg', 'panel', 'row', 'hover', 'selected'];
const pageRoles = ['bg', 'panel', 'row'];
const fillPageRoles = ['bg', 'panel'];

const descriptions = {
  'background.canvas': 'Page background behind all content.',
  'background.surface': 'Standard containers such as cards, panels, and sections.',
  'background.raised': 'Rows and controls that sit above a surface.',
  'background.hover': 'Hover state for rows and secondary controls.',
  'background.selected': 'Selected row or navigation item.',
  'background.backdrop': 'Scrim behind modal dialogs and blocking overlays.',
  'text.primary': 'Default body and label text.',
  'text.secondary': 'Supporting text, metadata, and muted icons.',
  'text.accent': 'Links, selected labels, and accent icons on neutral surfaces.',
  'text.accentHover': 'Hover state for accent text and links.',
  'text.onFill': 'Text and icons placed on a primary fill.',
  'fill.primary': 'High-prominence filled controls such as primary buttons.',
  'fill.primaryHover': 'Hover state for a primary fill.',
  'fill.neutral': 'Neutral indicators and muted fills.',
  'fill.warning': 'Warning indicators and progress bars.',
  'fill.danger': 'Destructive filled indicators.',
  'border.default': 'Dividers, input borders, and container outlines.',
  'border.strong': 'Control boundaries and scrollbar thumbs that need more emphasis.',
  'border.focus': 'Keyboard focus ring.',
  'status.success': 'Successful outcome text and icons.',
  'status.warning': 'Warning text and icons.',
  'status.danger': 'Error and destructive text and icons.'
};

function nearest(candidates, target, accept, label) {
  let best;
  for (const candidate of candidates) {
    if (accept && !accept(candidate)) continue;
    if (!best || Math.abs(candidate.lightness - target) < Math.abs(best.lightness - target) - 1e-9) best = candidate;
  }
  if (!best) throw new Error(`No ${label} step satisfies the usage policy`);
  return best;
}

/** Builds ramps, selects role steps under contrast constraints, and emits aliases. */
export function generateDesign({ recipe, config }) {
  const steps = rampSteps(recipe.resolution);
  const ramps = new Map();
  for (const color of recipe.colors) {
    const hue = keyHue(color.key);
    ramps.set(color.name, steps.map(step => {
      const rgb = rampColor(hue, color.saturation, step);
      return { step, rgb, lightness: lightnessOf(rgb) };
    }));
  }
  const used = new Map(recipe.colors.map(color => [color.name, new Set()]));
  const pick = (name, target, accept, role) => {
    const candidate = nearest(ramps.get(name), target, accept, role);
    used.get(name).add(candidate.step);
    return candidate;
  };

  const { lightness, minimumContrast } = config.color;
  const themes = {};
  for (const [mode, policy] of Object.entries(lightness)) {
    const values = {};
    for (const role of ['bg', 'panel', 'row', 'hover']) values[role] = { ...pick('neutral', policy[role], null, role), seed: 'neutral' };
    values.text = { ...pick(
      'neutral',
      policy.text,
      candidate => ['bg', 'panel', 'row', 'hover'].every(role => contrast(candidate.rgb, values[role].rgb) >= minimumContrast.text),
      'text',
    ), seed: 'neutral' };
    const selected = pick('brand', policy.selected,
      candidate => contrast(candidate.rgb, values.text.rgb) >= minimumContrast.text, 'selected');
    values.selected = { ...selected, seed: 'brand' };

    const allSurfaces = surfaceRoles.map(role => values[role].rgb);
    values.dim = { ...pick('neutral', policy.dim,
      candidate => allSurfaces.every(background => contrast(candidate.rgb, background) >= minimumContrast.text), 'dim'), seed: 'neutral' };
    values.accent = { ...pick('brand', policy.accent,
      candidate => allSurfaces.every(background => contrast(candidate.rgb, background) >= minimumContrast.text), 'accent text'), seed: 'brand' };
    values.accentHover = { ...pick('brand', policy.accentHover,
      candidate => allSurfaces.every(background => contrast(candidate.rgb, background) >= minimumContrast.text), 'accent text hover'), seed: 'brand' };
    values.borderStrong = { ...pick('neutral', policy.borderStrong,
      candidate => pageRoles.every(role => contrast(candidate.rgb, values[role].rgb) >= minimumContrast.control), 'border strong'), seed: 'neutral' };
    values.border = pick('neutral', policy.border, null, 'border');
    values.border = { ...values.border, seed: 'neutral' };

    const onFill = { ...pick('neutral', policy.onAccent, null, 'on-fill text'), seed: 'neutral' };
    const fillAccept = candidate => contrast(onFill.rgb, candidate.rgb) >= minimumContrast.text
      && fillPageRoles.every(role => contrast(candidate.rgb, values[role].rgb) >= minimumContrast.control);
    values.fillPrimary = { ...pick('brand', policy.fillPrimary, fillAccept, 'primary fill'), seed: 'brand' };
    values.fillPrimaryHover = { ...pick('brand', policy.fillPrimaryHover, fillAccept, 'primary fill hover'), seed: 'brand' };

    const statusAccept = candidate => allSurfaces.every(background => contrast(candidate.rgb, background) >= minimumContrast.text);
    values.statusWarning = { ...pick('warning', policy.status, statusAccept, 'warning status'), seed: 'warning' };
    values.statusDanger = { ...pick('danger', policy.status, statusAccept, 'danger status'), seed: 'danger' };
    const fillStatusAccept = candidate => pageRoles.every(role => contrast(candidate.rgb, values[role].rgb) >= minimumContrast.control);
    values.fillWarning = { ...pick('warning', policy.status, fillStatusAccept, 'warning fill'), seed: 'warning' };
    values.fillDanger = { ...pick('danger', policy.status, fillStatusAccept, 'danger fill'), seed: 'danger' };
    values.fillNeutral = { ...pick('neutral', policy.dim, fillStatusAccept, 'neutral fill'), seed: 'neutral' };

    values.onFill = onFill;
    values.focus = values.accent;
    values.success = values.accent;
    values.scrim = { rgb: rampColor(keyHue(recipe.colors.find(color => color.name === 'neutral').key), recipe.colors.find(color => color.name === 'neutral').saturation, 920), alpha: policy.scrim };
    themes[mode] = values;
  }

  const markers = () => ({ 'com.tessera.generation': { source: 'generation/recipes/color.json' } });
  const seed = name => recipe.colors.find(color => color.name === name);
  const color = {};
  for (const [name, stepsUsed] of used) {
    const key = seed(name);
    const ramp = new Map(ramps.get(name).map(candidate => [candidate.step, candidate]));
    const group = { $root: { $value: { colorSpace: key.key.colorSpace, components: key.key.components, hex: key.key.hex } } };
    for (const step of [...stepsUsed].sort((a, b) => a - b)) group[String(step)] = { $value: { colorSpace: 'srgb', components: ramp.get(step).rgb } };
    color[name] = group;
  }

  const primitive = {
    $schema: 'https://designtokens.org/schemas/2025.10/format.json',
    $description: 'Generated OKHSL color scales. Edit generation/recipes/color.json or generation/config.json, then run npm run generate.',
    primitive: { $extensions: markers(), color: { $type: 'color', ...color } },
  };

  const ref = value => `{primitive.color.${value.seed}.${value.step}}`;
  const literal = value => ({ $value: { colorSpace: 'srgb', components: value.rgb, alpha: value.alpha } });
  const documents = {};
  for (const [mode, values] of Object.entries(themes)) {
    const group = (description, entries) => ({
      $type: 'color',
      $description: description,
      ...Object.fromEntries(entries.map(([path, entry]) => [path.split('.').at(-1), { ...entry, $description: descriptions[path] }])),
    });
    documents[mode] = {
      $schema: 'https://designtokens.org/schemas/2025.10/format.json',
      $description: `Generated ${mode} semantic roles. Edit generation/recipes/color.json or generation/config.json, then run npm run generate.`,
      $extensions: { 'com.tessera.generation': { mode, source: 'generation/recipes/color.json' } },
      background: group('Surface roles.', [
        ['background.canvas', { $value: ref(values.bg) }],
        ['background.surface', { $value: ref(values.panel) }],
        ['background.raised', { $value: ref(values.row) }],
        ['background.hover', { $value: ref(values.hover) }],
        ['background.selected', { $value: ref(values.selected) }],
        ['background.backdrop', literal(values.scrim)],
      ]),
      text: group('Text roles.', [
        ['text.primary', { $value: ref(values.text) }],
        ['text.secondary', { $value: ref(values.dim) }],
        ['text.accent', { $value: ref(values.accent) }],
        ['text.accentHover', { $value: ref(values.accentHover) }],
        ['text.onFill', { $value: ref(values.onFill) }],
      ]),
      fill: group('Fill roles.', [
        ['fill.primary', { $value: ref(values.fillPrimary) }],
        ['fill.primaryHover', { $value: ref(values.fillPrimaryHover) }],
        ['fill.neutral', { $value: ref(values.fillNeutral) }],
        ['fill.warning', { $value: ref(values.fillWarning) }],
        ['fill.danger', { $value: ref(values.fillDanger) }],
      ]),
      border: group('Border roles.', [
        ['border.default', { $value: ref(values.border) }],
        ['border.strong', { $value: ref(values.borderStrong) }],
        ['border.focus', { $value: ref(values.focus) }],
      ]),
      status: group('Status roles.', [
        ['status.success', { $value: ref(values.success) }],
        ['status.warning', { $value: ref(values.statusWarning) }],
        ['status.danger', { $value: ref(values.statusDanger) }],
      ]),
    };
  }

  return { primitive, themes: documents };
}
