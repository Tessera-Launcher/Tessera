export const FONT_BASE_PX = 12;
export const FONT_NOTES_PER_INTERVAL = 5;
export const DEFAULT_LINE_HEIGHT = {
  anchors: [{ fontSizePx: 10, ratio: 1.2 }, { fontSizePx: 16, ratio: 1.45 }, { fontSizePx: 84, ratio: 1.1 }],
  gridTiers: [{ minFontSizePx: 0, gridPx: 4 }, { minFontSizePx: 48, gridPx: 8 }],
};

/** Dimension scale: 8px base, 25 steps per pixel. */
export function dimensionPx(step, basePx = 8) {
  return (step * basePx) / 200;
}

/** Classical font scale: 12px base, ratio 2, five notes per interval. */
export function fontSizePx(step) {
  return Math.round(FONT_BASE_PX * 2 ** ((step - 200) / (100 * FONT_NOTES_PER_INTERVAL)));
}

/** Radius scale: base px, doubling every 100 steps. */
export function radiusPx(step, basePx = 3) {
  return basePx * 2 ** ((step - 100) / 100);
}

/** Border width scale: base px, doubling every 100 steps. */
export function borderWidthPx(step, basePx = 1) {
  return basePx * 2 ** ((step - 100) / 100);
}

export function lineHeightRatio(size, config = DEFAULT_LINE_HEIGHT) {
  const anchors = config.anchors;
  const first = anchors[0];
  if (size <= first.fontSizePx) return first.ratio;
  for (let index = 1; index < anchors.length; index++) {
    const lower = anchors[index - 1];
    const upper = anchors[index];
    if (size <= upper.fontSizePx) {
      const progress = Math.log(size / lower.fontSizePx) / Math.log(upper.fontSizePx / lower.fontSizePx);
      return lower.ratio + (upper.ratio - lower.ratio) * progress;
    }
  }
  return anchors.at(-1).ratio;
}

/** Line height snapped to the 4px grid below 48px and 8px above. */
export function lineHeightPx(size, config = DEFAULT_LINE_HEIGHT) {
  let grid = config.gridTiers[0].gridPx;
  for (const tier of config.gridTiers.slice(1)) {
    if (size < tier.minFontSizePx) break;
    grid = tier.gridPx;
  }
  const nearest = Math.round((size * lineHeightRatio(size, config)) / grid) * grid;
  const minimum = Math.ceil(size / grid) * grid;
  return Math.max(nearest, minimum);
}

export const round = (value, precision = 6) => Number(value.toFixed(precision));
