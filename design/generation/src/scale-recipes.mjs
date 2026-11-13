import { readFile } from 'node:fs/promises';
import Ajv from 'ajv';

const names = ['dimension', 'radius', 'typography', 'duration', 'borderWidth', 'opacity'];
const ajv = new Ajv({ allErrors: true, strictSchema: true, strictTypes: false });

async function readJson(path) {
  return JSON.parse(await readFile(new URL(path, import.meta.url), 'utf8'));
}

/** Loads and validates every scale recipe before generation. */
export async function loadScaleRecipes() {
  const recipes = {};
  for (const name of names) {
    const recipe = await readJson(`../recipes/${name}.json`);
    const schema = await readJson(`../schemas/${name}.recipe.schema.json`);
    const validate = ajv.compile(schema);
    if (!validate(recipe)) {
      const details = (validate.errors ?? []).map(error => `${error.instancePath || '/'} ${error.message}`).join('\n');
      throw new Error(`generation/recipes/${name}.json is invalid:\n${details}`);
    }
    recipes[name] = recipe;
  }
  return recipes;
}
