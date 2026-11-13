export function generateOpacity(recipe) {
  if (!recipe.steps.includes(recipe.role.step)) throw new Error(`opacity.${recipe.role.name} uses unknown step`);
  const opacity = { $type: 'number' };
  const number = { $type: 'number' };
  for (const step of recipe.steps) number[String(step)] = { $value: step / 1000 };
  opacity[recipe.role.name] = { $value: `{primitive.number.opacity.${recipe.role.step}}` };
  return { semantic: { opacity }, primitive: { primitive: { number: { $type: 'number', opacity: number } } } };
}
