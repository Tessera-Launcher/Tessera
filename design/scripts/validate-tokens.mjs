import { createHash } from 'node:crypto';
import { readdir, readFile } from 'node:fs/promises';
import { relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import Ajv from 'ajv';
import addFormats from 'ajv-formats';

// Standards compliance is a separate gate from Terrazzo's parser: the official
// JSON Schema rejects format violations that Terrazzo's lint rules deliberately
// skip, so both layers run. The schemas below are unmodified official files.
const root = fileURLToPath(new URL('../../', import.meta.url));
const version = '2025.10';
const directory = resolve(root, `design/schemas/dtcg/${version}`);
const names = ['format', 'resolver'];

let validators;

async function compile() {
  const manifest = JSON.parse(await readFile(resolve(directory, 'manifest.json'), 'utf8'));
  if (manifest.version !== version) throw new Error(`manifest.json declares DTCG ${manifest.version}, expected ${version}`);
  const compiled = {};
  for (const name of names) {
    const content = await readFile(resolve(directory, `${name}.json`));
    const expected = manifest.schemas?.[name]?.sha256;
    const actual = createHash('sha256').update(content).digest('hex');
    if (!expected) throw new Error(`manifest.json is missing the ${name} checksum`);
    if (expected !== actual) throw new Error(`design/schemas/dtcg/${version}/${name}.json does not match its manifest checksum; re-download the official schema instead of editing it`);
    // The official files are JSON Schema draft-07 and bundle their own references.
    const ajv = new Ajv({ allErrors: true, strictSchema: true, strictTypes: false, validateFormats: true });
    addFormats(ajv);
    compiled[name] = ajv.compile(JSON.parse(content.toString('utf8')));
  }
  validators = compiled;
  return validators;
}

export async function validateDocument(type, document) {
  const validate = (validators ?? await compile())[type];
  if (!validate) throw new Error(`Unknown DTCG document type: ${type}`);
  if (validate(document)) return [];
  return (validate.errors ?? []).map(error => `${error.instancePath || '/'}: ${error.message} (${error.schemaPath})`);
}

async function walk(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) files.push(...await walk(path));
    else files.push(path);
  }
  return files;
}

async function projectDocuments() {
  const tokens = (await walk(resolve(root, 'design/tokens')))
    .filter(file => file.endsWith('.tokens.json'))
    .map(file => relative(root, file).replaceAll('\\', '/'))
    .sort();
  return { format: tokens, resolver: ['design/tokens/tessera.resolver.json'] };
}

async function main() {
  const documents = await projectDocuments();
  let failed = false;
  for (const [type, files] of Object.entries(documents)) {
    for (const file of files) {
      const errors = await validateDocument(type, JSON.parse(await readFile(resolve(root, file), 'utf8')));
      if (!errors.length) continue;
      failed = true;
      for (const error of errors) console.error(`${file}${error}`);
    }
  }
  if (failed) {
    process.exitCode = 1;
    return;
  }
  const sizes = Object.entries(documents).map(([type, files]) => `${files.length} ${type}`).join(' + ');
  console.log(`DTCG ${version}: ${sizes} valid (unofficial extensions rejected)`);
}

if (process.argv[1]?.replaceAll('\\', '/').endsWith('/design/scripts/validate-tokens.mjs')) main().catch(error => {
  console.error(error.message);
  process.exitCode = 1;
});
