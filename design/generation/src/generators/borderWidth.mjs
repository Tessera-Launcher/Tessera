import { borderWidthPx } from '../scales.mjs';

export function generateBorderWidth(recipe) {
  const steps = new Set(recipe.steps);
  const stroke = { $type: 'dimension' };
  for (const [role, step] of Object.entries(recipe.roles)) {
    if (!steps.has(step)) throw new Error(`stroke.${role} uses unknown step ${step}`);
    stroke[role] = { $value: { value: borderWidthPx(step, recipe.basePx), unit: 'px' } };
  }
  return { stroke };
}
