import Color from 'colorjs.io';

// OKHSL scale model: lightness falls linearly from white at step 0 to black at
// step 1000; saturation follows a Gaussian bell centred on the middle step so
// the ends stay neutral and mid-tones carry the most colour.
const CENTER = 0.5;
const SIGMA = 0.25;
const gaussian = x => Math.exp(-((x - CENTER) ** 2) / (2 * SIGMA ** 2));
const EDGE = gaussian(0);
const bell = position => (gaussian(position) - EDGE) / (1 - EDGE);

export const quantize = rgb => rgb.map(c => Math.round(Math.min(1, Math.max(0, c)) * 255) / 255);
export const toHex = rgb => '#' + rgb.map(c => Math.round(c * 255).toString(16).padStart(2, '0')).join('');

export function rampSteps(resolution) {
  const steps = [];
  for (let step = resolution; step < 1000; step += resolution) steps.push(step);
  return steps;
}

export function keyHue(key) {
  const [, , hue] = new Color(key.colorSpace, key.components).to('oklch').coords;
  return Number.isFinite(hue) ? hue : 0;
}

/** One OKHSL ramp entry, quantized to the 8-bit sRGB channels both outputs use. */
export function rampColor(hue, saturation, step) {
  const position = step / 1000;
  const value = saturation.min + (saturation.max - saturation.min) * bell(position);
  const rgb = new Color('okhsl', [hue, value, 1 - position]).to('srgb').toGamut({ method: 'css' });
  return quantize(rgb.coords);
}

export const lightnessOf = rgb => new Color('srgb', rgb).to('oklch').coords[0];
