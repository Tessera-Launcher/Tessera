import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { generateDesign } from '../generation/src/color.mjs';
import { loadConfig } from '../generation/src/config.mjs';
import { loadRecipe } from '../generation/src/recipe.mjs';
import { loadScaleRecipes } from '../generation/src/scale-recipes.mjs';
import { generateDimension } from '../generation/src/generators/dimension.mjs';
import { generateRadius } from '../generation/src/generators/radius.mjs';
import { generateTypography } from '../generation/src/generators/typography.mjs';
import { generateDuration } from '../generation/src/generators/duration.mjs';
import { generateBorderWidth } from '../generation/src/generators/borderWidth.mjs';
import { generateOpacity } from '../generation/src/generators/opacity.mjs';

const design = fileURLToPath(new URL('../', import.meta.url));
const check = process.argv.includes('--check');
const schema = 'https://designtokens.org/schemas/2025.10/format.json';
const marker = sources => ({ 'com.tessera.generation': { source: sources.map(source => `generation/recipes/${source}.json`) } });

const color = generateDesign({ recipe: await loadRecipe(), config: await loadConfig() });
const recipes = await loadScaleRecipes();
const dimension = generateDimension(recipes.dimension);
const { radius } = generateRadius(recipes.radius);
const { stroke } = generateBorderWidth(recipes.borderWidth);
const { font, lineHeights } = generateTypography(recipes.typography);
const { motion } = generateDuration(recipes.duration);
const opacity = generateOpacity(recipes.opacity);

const outputs = new Map([
  ['tokens/foundation/dimension.tokens.json', {
    $schema: schema,
    $description: 'Generated dimensions. Edit generation/recipes/{dimension,radius,typography,borderWidth}.json, then run npm run generate.',
    $extensions: marker(['dimension', 'radius', 'typography', 'borderWidth']),
    space: dimension.space,
    layout: dimension.layout,
    radius,
    stroke,
    font,
  }],
  ['tokens/foundation/number.tokens.json', {
    $schema: schema,
    $description: 'Generated numbers. Edit generation/recipes/{typography,opacity}.json, then run npm run generate.',
    $extensions: marker(['typography', 'opacity']),
    ...opacity.primitive,
    font: lineHeights,
  }],
  ['tokens/foundation/duration.tokens.json', {
    $schema: schema,
    $description: 'Generated durations. Edit generation/recipes/duration.json, then run npm run generate.',
    $extensions: marker(['duration']),
    motion,
  }],
  ['tokens/foundation/color.tokens.json', color.primitive],
  ['tokens/semantic/number.tokens.json', {
    $schema: schema,
    $description: 'Mode-independent numeric roles.',
    $extensions: marker(['opacity']),
    ...opacity.semantic,
  }],
  ...Object.entries(color.themes).map(([mode, document]) => [`tokens/semantic/color/${mode}.tokens.json`, document]),
]);

for (const [path, document] of outputs) {
  const target = resolve(design, path);
  const content = JSON.stringify(document, null, 2) + '\n';
  if (check) {
    if (await readFile(target, 'utf8') !== content) throw new Error(`design/${path} is stale. Run npm run generate.`);
  } else {
    await mkdir(dirname(target), { recursive: true });
    await writeFile(target, content);
  }
}

console.log(`${check ? 'Checked' : 'Generated'} ${Object.keys(color.themes).length} color schemes and ${Object.keys(recipes).length} scale recipes.`);
