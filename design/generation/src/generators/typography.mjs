import { fontSizePx, lineHeightPx, round } from '../scales.mjs';

export function generateTypography(recipe) {
  const fonts = {};
  const lineHeights = {};
  for (const [role, step] of Object.entries(recipe.roles)) {
    if (!recipe.steps.includes(step)) throw new Error(`font.${role} uses unknown step ${step}`);
    const size = fontSizePx(step);
    const height = lineHeightPx(size, recipe.lineHeight);
    fonts[role] = { $type: 'dimension', $value: { value: size, unit: 'px' } };
    const key = role === 'body' ? 'lineHeight' : 'lineHeight' + role[0].toUpperCase() + role.slice(1);
    lineHeights[key] = { $type: 'number', $value: round(height / size) };
  }
  return { font: fonts, lineHeights };
}
