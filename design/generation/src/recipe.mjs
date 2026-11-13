import { readFile } from 'node:fs/promises';
import Ajv from 'ajv';
import Color from 'colorjs.io';

const schema = JSON.parse(await readFile(new URL('../schemas/color.recipe.schema.json', import.meta.url), 'utf8'));
const validate = new Ajv({ allErrors: true, strictSchema: true, strictTypes: false }).compile(schema);

/** Rejects recipes the generator cannot turn into a complete scale. */
export function validateRecipe(recipe) {
  if (!validate(recipe)) {
    const details = (validate.errors ?? []).map(error => `${error.instancePath || '/'} ${error.message}`).join('\n');
    throw new Error(`design/generation/recipes/color.json is invalid:\n${details}`);
  }
  const names = new Set();
  for (const color of recipe.colors) {
    if (names.has(color.name)) throw new Error(`Duplicate color name: ${color.name}`);
    names.add(color.name);
    const { min, max } = color.saturation;
    if (min > max) throw new Error(`Color ${color.name} saturation min must not exceed max`);
    if (!new Color(color.key.colorSpace, color.key.components).inGamut('srgb', { epsilon: 1e-4 })) {
      throw new Error(`Color ${color.name} key is outside the sRGB gamut`);
    }
  }
}

export async function loadRecipe() {
  const recipe = JSON.parse(await readFile(new URL('../recipes/color.json', import.meta.url), 'utf8'));
  validateRecipe(recipe);
  return recipe;
}
