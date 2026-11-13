export function generateDuration(recipe) {
  const steps = new Set(recipe.steps);
  const motion = { $type: 'duration' };
  for (const [role, step] of Object.entries(recipe.roles)) {
    if (!steps.has(step)) throw new Error(`motion.${role} uses unknown step ${step}`);
    if (step % 50 !== 0) throw new Error(`motion.${role} must be a 50ms step`);
    motion[role] = { $value: { value: step, unit: 'ms' } };
  }
  return { motion };
}
