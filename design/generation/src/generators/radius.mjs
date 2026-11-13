import { radiusPx } from '../scales.mjs';

export function generateRadius(recipe) {
  const steps = new Set(recipe.steps);
  const radius = { $type: 'dimension' };
  for (const [role, step] of Object.entries(recipe.roles)) {
    if (!steps.has(step)) throw new Error(`radius.${role} uses unknown step ${step}`);
    radius[role] = { $value: { value: radiusPx(step, recipe.basePx), unit: 'px' } };
  }
  radius.pill = { $value: { value: recipe.fullPx, unit: 'px' } };
  return { radius };
}
