import { dimensionPx } from '../scales.mjs';

export function generateDimension(recipe) {
  const dimension = step => {
    if (!Number.isInteger(step) || step % 25 !== 0) throw new Error(`dimension step ${step} must be a multiple of 25`);
    const value = dimensionPx(step, recipe.basePx);
    if (!Number.isInteger(value)) throw new Error(`dimension step ${step} must land on a whole pixel`);
    return { $value: { value, unit: 'px' } };
  };
  return {
    space: { $type: 'dimension', ...Object.fromEntries(Object.entries(recipe.spacing).map(([role, step]) => [role, dimension(step)])) },
    layout: { $type: 'dimension', ...Object.fromEntries(Object.entries(recipe.layout).map(([role, step]) => [role, dimension(step)])) },
  };
}
