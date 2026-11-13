import { readFile } from 'node:fs/promises';
import Ajv2020 from 'ajv/dist/2020.js';

const schema = JSON.parse(await readFile(new URL('../config.schema.json', import.meta.url), 'utf8'));
const validate = new Ajv2020({ allErrors: true, strict: true }).compile(schema);

/** Rejects policy edits that would make the generator read impossible tones. */
export function validateConfig(config) {
  if (validate(config)) return;
  const details = (validate.errors ?? []).map(error => `${error.instancePath || '/'} ${error.message}`).join('\n');
  throw new Error(`design/generation/config.json is invalid:\n${details}`);
}

export async function loadConfig() {
  const config = JSON.parse(await readFile(new URL('../config.json', import.meta.url), 'utf8'));
  validateConfig(config);
  return config;
}
