#!/usr/bin/env node
/**
 * Renders the EverTranscript brand masters into every platform format.
 *
 * One mark (`src/mark.svg`, with `src/mark-small.svg` for anything 32 px and
 * under) becomes: the macOS `.icns`, the Windows `.ico`, the iOS
 * `AppIcon.appiconset` (with the iOS 18 dark and tinted appearances), the
 * Android adaptive icon, the web favicon set, the four template glyphs the
 * menu bar shows, and the lockups. Nothing here needs a tool outside this
 * repo's toolchain: resvg rasterizes, and the containers that have no npm
 * writer worth taking (`.icns`, `.ico`, the menu bar's multi-representation
 * `.tiff`) are written by hand below — each is a header and a table.
 *
 * Deterministic on purpose: no text is ever rendered (`loadSystemFonts` is
 * off — the wordmark is outlined before it gets here), every size is
 * rasterized from vector rather than downscaled, and files are only
 * rewritten when their bytes change. Re-running must produce no diff.
 *
 *   pnpm -C brand render                 every output
 *   pnpm -C brand render:explorations    the concept contact sheet
 */

import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

import { Resvg } from "@resvg/resvg-js";
import opentype from "opentype.js";

const BRAND = dirname(fileURLToPath(import.meta.url));
const REPO = join(BRAND, "..");
const SRC = join(BRAND, "src");
const OUT = join(BRAND, "generated");

/** The palette. `brand/README.md` is the human-readable copy of this table. */
export const COLOR = {
  teal400: "#158580",
  teal500: "#0F6E6A",
  teal700: "#094F4C",
  tealDeep: "#06302E",
  paper: "#F5F1E8",
  ink: "#1F1D1B",
  record: "#E5484D",
};

/** The mark's ink spans 32..224 of its 256 grid: three quarters of the box. */
const INK = 192 / 256;
/** How much of a tile's width the mark's ink takes. */
const GLYPH_FRACTION = 0.56;

// ---------------------------------------------------------------------------
// Rasterizing
// ---------------------------------------------------------------------------

/** Renders an SVG string at `width` pixels; returns resvg's image object. */
function rasterize(svg, width, options = {}) {
  const resvg = new Resvg(svg, {
    fitTo: { mode: "width", value: width },
    font: { loadSystemFonts: false },
    logLevel: "error",
    ...options,
  });
  return resvg.render();
}

const png = (svg, width) => rasterize(svg, width).asPng();

/** Reads a mark master and returns its inner markup, ready to be placed. */
function loadGlyph(path) {
  const text = readFileSync(path, "utf8");
  if (/<text[\s>]/.test(text)) {
    throw new Error(`${path}: text must be converted to outlines`);
  }
  const inner = text.match(/<svg[^>]*>([\s\S]*)<\/svg>/);
  if (!inner) throw new Error(`${path}: not an SVG document`);
  return inner[1].trim();
}

/**
 * Places a 256-grid glyph so that its ink is `inkWidth` wide, centred at
 * (cx, cy). Masters draw in `currentColor`, which is what makes one drawing
 * serve as paper on teal, ink on paper, and a template.
 */
function place(glyph, { cx, cy, inkWidth, color, opacity = 1 }) {
  const box = inkWidth / INK;
  const scale = box / 256;
  const x = cx - box / 2;
  const y = cy - box / 2;
  const alpha = opacity === 1 ? "" : ` opacity="${opacity}"`;
  return `<g color="${color}"${alpha} transform="translate(${x} ${y}) scale(${scale})">${glyph}</g>`;
}

const gradient = (id, top, bottom) =>
  `<linearGradient id="${id}" x1="0" y1="0" x2="0" y2="1">` +
  `<stop offset="0" stop-color="${top}"/><stop offset="1" stop-color="${bottom}"/></linearGradient>`;

/**
 * A tile: the coloured shape the mark sits on.
 *
 * `inset` and `radius` are fractions of the canvas, so the same call shapes
 * the macOS icon (inset, large radius, shadow), the full-bleed rounded
 * square Windows and the web use, the circle Android's round launcher wants,
 * and the plain square iOS masks itself.
 */
function tile(glyph, options = {}) {
  const {
    size = 1024,
    inset = 0,
    radius = 0,
    shadow = false,
    background = true,
    dark = false,
    glyphFraction = GLYPH_FRACTION,
    glyphColor = COLOR.paper,
    glyphOpacity = 1,
  } = options;
  const edge = size * inset;
  const side = size - 2 * edge;
  const rx = side * radius;
  const [top, bottom] = dark ? [COLOR.teal700, COLOR.tealDeep] : [COLOR.teal400, COLOR.teal700];
  const blur = size * 0.0234;
  const drop = size * 0.0117;
  const defs =
    gradient("tile", top, bottom) +
    (shadow
      ? `<filter id="shadow" x="-25%" y="-25%" width="150%" height="150%">` +
        `<feGaussianBlur in="SourceAlpha" stdDeviation="${blur}" result="blur"/>` +
        `<feOffset in="blur" dy="${drop}" result="offset"/>` +
        `<feFlood flood-color="#000000" flood-opacity="0.3" result="colour"/>` +
        `<feComposite in="colour" in2="offset" operator="in"/></filter>`
      : "");
  const shape = `x="${edge}" y="${edge}" width="${side}" height="${side}" rx="${rx}"`;
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${size} ${size}">` +
    `<defs>${defs}</defs>` +
    (shadow ? `<rect ${shape} fill="#000000" filter="url(#shadow)"/>` : "") +
    (background ? `<rect ${shape} fill="url(#tile)"/>` : "") +
    place(glyph, {
      cx: size / 2,
      cy: size / 2,
      inkWidth: side * glyphFraction,
      color: glyphColor,
      opacity: glyphOpacity,
    }) +
    `</svg>`
  );
}

/** The macOS icon: Apple's inset rounded square with its soft shadow. */
const macosTile = (glyph, extra = {}) => tile(glyph, { inset: 100 / 1024, radius: 185 / 824, shadow: true, ...extra });
/** Full-bleed rounded square: Windows, the favicon, PWA icons. */
const roundedTile = (glyph, extra = {}) => tile(glyph, { radius: 0.2, ...extra });
/** Full-bleed square, the platform masks it: iOS, apple-touch-icon, Play Store. */
const squareTile = (glyph, extra = {}) => tile(glyph, { radius: 0, ...extra });
/** A circle: Android's round legacy launcher icon. */
const circleTile = (glyph, extra = {}) => tile(glyph, { radius: 0.5, ...extra });

/**
 * The menu bar glyph: black with alpha on an 18 pt canvas, ink 16 pt wide.
 * macOS renders a template image itself — black on a light bar, white on a
 * dark one, dimmed when the app is inactive — which is why there is no
 * colour here. States add a badge at the lower right.
 */
function trayGlyph(glyph, { state = "ready", color = "#000000" } = {}) {
  const S = 18;
  const badge = {
    ready: "",
    // Recording: a solid dot, with a clear ring so it separates from the
    // mark instead of merging into it at 1x.
    recording: `<circle cx="14.5" cy="14.5" r="3.25" fill="${color}"/>`,
    // Attention: the same dot, hollow — something needs the Operator.
    attention:
      `<circle cx="14.5" cy="14.5" r="2.75" fill="none" stroke="${color}" stroke-width="1.5"/>`,
    busy: "",
  }[state];
  if (badge === undefined) throw new Error(`unknown tray state ${state}`);
  const opacity = state === "busy" ? 0.45 : 1;
  // A badge needs room: the mark steps back a little when one is shown.
  const withBadge = state === "recording" || state === "attention";
  const inkWidth = withBadge ? 13.5 : 16;
  const cx = withBadge ? 7.75 : 9;
  const cy = withBadge ? 8.25 : 9;
  const knockout = withBadge
    ? `<mask id="knock"><rect width="${S}" height="${S}" fill="#fff"/><circle cx="14.5" cy="14.5" r="4.75" fill="#000"/></mask>`
    : "";
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${S} ${S}">` +
    `<defs>${knockout}</defs>` +
    (withBadge ? `<g mask="url(#knock)">` : "") +
    place(glyph, { cx, cy, inkWidth, color, opacity }) +
    (withBadge ? `</g>` : "") +
    badge +
    `</svg>`
  );
}

// ---------------------------------------------------------------------------
// Stroke outlining
// ---------------------------------------------------------------------------

/**
 * The masters are drawn as strokes — one width, round caps and joins — which
 * is the honest way to author a monoline mark. Not every consumer renders
 * strokes (Icon Composer fills the path instead), so the same drawing is
 * also produced as filled outlines: each subpath is sampled, offset to both
 * sides, closed with round caps, and filled nonzero so overlaps merge.
 */

/** Parses the SVG path commands the masters use into sampled polylines. */
function samplePath(d, step = 1) {
  const tokens = d.match(/[A-Za-z]|-?(?:\d+\.?\d*|\.\d+)(?:e[-+]?\d+)?/g) ?? [];
  const subpaths = [];
  let points = null;
  let current = [0, 0];
  let control = null;
  let command = null;
  let i = 0;
  const number = () => Number(tokens[i++]);
  const lineTo = (to) => {
    const [x0, y0] = current;
    const length = Math.hypot(to[0] - x0, to[1] - y0);
    const n = Math.max(1, Math.ceil(length / step));
    for (let k = 1; k <= n; k += 1) points.push([x0 + ((to[0] - x0) * k) / n, y0 + ((to[1] - y0) * k) / n]);
    current = to;
  };
  const cubicTo = (c1, c2, to) => {
    const [x0, y0] = current;
    const length = Math.hypot(c1[0] - x0, c1[1] - y0) + Math.hypot(c2[0] - c1[0], c2[1] - c1[1]) + Math.hypot(to[0] - c2[0], to[1] - c2[1]);
    const n = Math.max(2, Math.ceil(length / step));
    for (let k = 1; k <= n; k += 1) {
      const t = k / n;
      const u = 1 - t;
      points.push([
        u * u * u * x0 + 3 * u * u * t * c1[0] + 3 * u * t * t * c2[0] + t * t * t * to[0],
        u * u * u * y0 + 3 * u * u * t * c1[1] + 3 * u * t * t * c2[1] + t * t * t * to[1],
      ]);
    }
    control = c2;
    current = to;
  };
  // SVG's endpoint-to-centre arc conversion (implementation notes F.6.5).
  const arcTo = (rx, ry, rotation, large, sweep, to) => {
    const [x1, y1] = current;
    const [x2, y2] = to;
    const phi = (rotation * Math.PI) / 180;
    const cos = Math.cos(phi);
    const sin = Math.sin(phi);
    const dx = (x1 - x2) / 2;
    const dy = (y1 - y2) / 2;
    const x1p = cos * dx + sin * dy;
    const y1p = -sin * dx + cos * dy;
    let RX = Math.abs(rx);
    let RY = Math.abs(ry);
    const lambda = (x1p * x1p) / (RX * RX) + (y1p * y1p) / (RY * RY);
    if (lambda > 1) {
      RX *= Math.sqrt(lambda);
      RY *= Math.sqrt(lambda);
    }
    const sign = large === sweep ? -1 : 1;
    const num = RX * RX * RY * RY - RX * RX * y1p * y1p - RY * RY * x1p * x1p;
    const den = RX * RX * y1p * y1p + RY * RY * x1p * x1p;
    const coef = sign * Math.sqrt(Math.max(0, num / den));
    const cxp = (coef * RX * y1p) / RY;
    const cyp = (-coef * RY * x1p) / RX;
    const cx = cos * cxp - sin * cyp + (x1 + x2) / 2;
    const cy = sin * cxp + cos * cyp + (y1 + y2) / 2;
    const angle = (ux, uy, vx, vy) => {
      const dot = ux * vx + uy * vy;
      const len = Math.hypot(ux, uy) * Math.hypot(vx, vy);
      let a = Math.acos(Math.max(-1, Math.min(1, dot / len)));
      if (ux * vy - uy * vx < 0) a = -a;
      return a;
    };
    const theta = angle(1, 0, (x1p - cxp) / RX, (y1p - cyp) / RY);
    let delta = angle((x1p - cxp) / RX, (y1p - cyp) / RY, (-x1p - cxp) / RX, (-y1p - cyp) / RY);
    if (!sweep && delta > 0) delta -= 2 * Math.PI;
    if (sweep && delta < 0) delta += 2 * Math.PI;
    const n = Math.max(2, Math.ceil((Math.abs(delta) * Math.max(RX, RY)) / step));
    for (let k = 1; k <= n; k += 1) {
      const t = theta + (delta * k) / n;
      const ex = RX * Math.cos(t);
      const ey = RY * Math.sin(t);
      points.push([cos * ex - sin * ey + cx, sin * ex + cos * ey + cy]);
    }
    current = to;
  };
  while (i < tokens.length) {
    if (/[A-Za-z]/.test(tokens[i])) command = tokens[i++];
    switch (command) {
      case "M":
        current = [number(), number()];
        points = [current];
        subpaths.push(points);
        control = null;
        command = "L";
        break;
      case "L":
        lineTo([number(), number()]);
        control = null;
        break;
      case "H":
        lineTo([number(), current[1]]);
        control = null;
        break;
      case "V":
        lineTo([current[0], number()]);
        control = null;
        break;
      case "C":
        cubicTo([number(), number()], [number(), number()], [number(), number()]);
        break;
      case "S": {
        const c1 = control ? [2 * current[0] - control[0], 2 * current[1] - control[1]] : current;
        cubicTo(c1, [number(), number()], [number(), number()]);
        break;
      }
      case "A": {
        const rx = number();
        const ry = number();
        const rotation = number();
        const large = number() !== 0;
        const sweep = number() !== 0;
        arcTo(rx, ry, rotation, large, sweep, [number(), number()]);
        control = null;
        break;
      }
      default:
        throw new Error(`outline: unsupported path command ${command} (masters use absolute M L H V C S A)`);
    }
  }
  return subpaths;
}

/** The filled outline of one open polyline stroked at `width`, round-capped. */
function outlinePolyline(points, width) {
  const r = width / 2;
  const pts = points.filter((p, k) => k === 0 || Math.hypot(p[0] - points[k - 1][0], p[1] - points[k - 1][1]) > 1e-6);
  if (pts.length < 2) return "";
  const segments = pts.length - 1;
  const dir = (k) => {
    const [ax, ay] = pts[k];
    const [bx, by] = pts[k + 1];
    const len = Math.hypot(bx - ax, by - ay);
    return [(bx - ax) / len, (by - ay) / len];
  };
  const left = [];
  const right = [];
  const arc = (centre, from, to, steps) => {
    // Points on the circle around `centre` sweeping from unit vector `from` to `to` the short way.
    const out = [];
    const a0 = Math.atan2(from[1], from[0]);
    let a1 = Math.atan2(to[1], to[0]);
    while (a1 - a0 > Math.PI) a1 -= 2 * Math.PI;
    while (a1 - a0 < -Math.PI) a1 += 2 * Math.PI;
    for (let k = 0; k <= steps; k += 1) {
      const a = a0 + ((a1 - a0) * k) / steps;
      out.push([centre[0] + r * Math.cos(a), centre[1] + r * Math.sin(a)]);
    }
    return out;
  };
  for (let k = 0; k < segments; k += 1) {
    const [tx, ty] = dir(k);
    const n = [-ty, tx];
    if (k > 0) {
      // Round join: on the outer side of a turn the offsets fan apart.
      const [px, py] = dir(k - 1);
      const pn = [-py, px];
      const turn = px * ty - py * tx;
      if (Math.abs(turn) > 1e-9) {
        const joinSteps = Math.max(1, Math.ceil((Math.abs(Math.asin(Math.max(-1, Math.min(1, turn)))) * r) / 1));
        if (turn < 0) left.push(...arc(pts[k], pn, n, joinSteps));
        else right.push(...arc(pts[k], [-pn[0], -pn[1]], [-n[0], -n[1]], joinSteps));
      }
    }
    left.push([pts[k][0] + n[0] * r, pts[k][1] + n[1] * r], [pts[k + 1][0] + n[0] * r, pts[k + 1][1] + n[1] * r]);
    right.push([pts[k][0] - n[0] * r, pts[k][1] - n[1] * r], [pts[k + 1][0] - n[0] * r, pts[k + 1][1] - n[1] * r]);
  }
  const capSteps = Math.max(8, Math.ceil((Math.PI * r) / 1));
  const [tx, ty] = dir(segments - 1);
  // `arc` takes the short way; a cap is exactly half a turn, so sweep it via the tangent.
  const half = (centre, from, via, steps) => {
    const out = [];
    for (let k = 0; k <= steps; k += 1) {
      const s = (Math.PI * k) / steps;
      const vx = from[0] * Math.cos(s) + via[0] * Math.sin(s);
      const vy = from[1] * Math.cos(s) + via[1] * Math.sin(s);
      out.push([centre[0] + r * vx, centre[1] + r * vy]);
    }
    return out;
  };
  const [sx, sy] = dir(0);
  const ring = [
    ...left,
    ...half(pts[segments], [-ty, tx], [tx, ty], capSteps),
    ...right.reverse(),
    ...half(pts[0], [sy, -sx], [-sx, -sy], capSteps),
  ];
  return `M${ring.map(([x, y]) => `${x.toFixed(2)},${y.toFixed(2)}`).join("L")}Z`;
}

/**
 * The master's inner markup with every stroked path replaced by its filled
 * outline. Reads the stroke width the masters declare on their group.
 */
function outlineGlyph(inner) {
  const width = Number((inner.match(/stroke-width="([\d.]+)"/) ?? [])[1]);
  if (!width) throw new Error("outline: the master declares no stroke-width");
  const fills = [];
  for (const [, d] of inner.matchAll(/<path[^>]*\sd="([^"]+)"/g)) {
    for (const polyline of samplePath(d)) fills.push(outlinePolyline(polyline, width));
  }
  return `<path fill="currentColor" fill-rule="nonzero" d="${fills.join("")}"/>`;
}

// ---------------------------------------------------------------------------
// Containers written by hand
// ---------------------------------------------------------------------------

/**
 * `.icns`: a four-byte magic, a big-endian total length, then chunks of
 * OSType + length + payload. Every chunk here is a PNG; the OSTypes below
 * are the ones a modern `iconutil` writes, verified against real files.
 */
function icns(entries) {
  const chunks = entries.map(({ type, data }) => {
    const header = Buffer.alloc(8);
    header.write(type, 0, "ascii");
    header.writeUInt32BE(8 + data.length, 4);
    return Buffer.concat([header, data]);
  });
  const body = Buffer.concat(chunks);
  const header = Buffer.alloc(8);
  header.write("icns", 0, "ascii");
  header.writeUInt32BE(8 + body.length, 4);
  return Buffer.concat([header, body]);
}

/** iconset name → icns OSType, as `iconutil` maps them. */
const ICNS_TYPES = [
  ["icon_16x16", 16, "icp4"],
  ["icon_16x16@2x", 32, "ic11"],
  ["icon_32x32", 32, "icp5"],
  ["icon_32x32@2x", 64, "ic12"],
  ["icon_128x128", 128, "ic07"],
  ["icon_128x128@2x", 256, "ic13"],
  ["icon_256x256", 256, "ic08"],
  ["icon_256x256@2x", 512, "ic14"],
  ["icon_512x512", 512, "ic09"],
  ["icon_512x512@2x", 1024, "ic10"],
];

/**
 * `.ico`: a six-byte directory header, a 16-byte entry per image, then the
 * images. PNG payloads are allowed at every size since Windows Vista, which
 * is what Granola's own `win-tray.ico` does.
 */
function ico(images) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(images.length, 4);
  const directory = Buffer.alloc(16 * images.length);
  let offset = header.length + directory.length;
  images.forEach(({ size, data }, index) => {
    const at = index * 16;
    directory[at] = size >= 256 ? 0 : size;
    directory[at + 1] = size >= 256 ? 0 : size;
    directory[at + 2] = 0; // colours in palette: none
    directory[at + 3] = 0; // reserved
    directory.writeUInt16LE(1, at + 4); // colour planes
    directory.writeUInt16LE(32, at + 6); // bits per pixel
    directory.writeUInt32LE(data.length, at + 8);
    directory.writeUInt32LE(offset, at + 12);
    offset += data.length;
  });
  return Buffer.concat([header, directory, ...images.map((image) => image.data)]);
}

const CRC_TABLE = new Uint32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (bytes) => {
  let c = 0xffffffff;
  for (const b of bytes) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const pngChunk = (type, data) => {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length, 0);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body), 0);
  return Buffer.concat([length, body, crc]);
};
const hexToRgb = (hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));

/**
 * A PNG with no alpha channel at all — colour type 2 — for the places that
 * refuse one: the App Store's 1024 icon, the Play Store's 512. resvg always
 * writes RGBA, so this composites its premultiplied pixels over `ground`
 * and encodes the three channels by hand. Unfiltered scanlines, deflated.
 */
function opaquePng(image, ground) {
  const { width, height, pixels } = image;
  const [gr, gg, gb] = hexToRgb(ground);
  const stride = width * 3 + 1;
  const rows = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y += 1) {
    rows[y * stride] = 0;
    for (let x = 0; x < width; x += 1) {
      const i = (y * width + x) * 4;
      const o = y * stride + 1 + x * 3;
      const under = 1 - pixels[i + 3] / 255;
      rows[o] = Math.round(pixels[i] + gr * under);
      rows[o + 1] = Math.round(pixels[i + 1] + gg * under);
      rows[o + 2] = Math.round(pixels[i + 2] + gb * under);
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // truecolour, no alpha
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", deflateSync(rows, { level: 9 })),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

/**
 * A multi-representation TIFF: one image per screen scale, distinguished
 * by resolution, which is how `NSImage` picks the 2x drawing on a Retina
 * display. Apple's own recipe for this is `tiffutil -cathidpicheck`; this
 * writes the same thing without needing macOS to build on.
 *
 * Little-endian, uncompressed RGBA with associated (premultiplied) alpha —
 * which is what resvg hands over — one strip per image.
 */
function tiff(reps) {
  const SHORT = 3;
  const LONG = 4;
  const RATIONAL = 5;
  const header = Buffer.alloc(8);
  header.write("II", 0, "ascii");
  header.writeUInt16LE(42, 2);
  header.writeUInt32LE(8, 4);
  const chunks = [header];
  let position = 8;
  reps.forEach(({ width, height, pixels, dpi }, index) => {
    const tags = 14;
    const ifdLength = 2 + tags * 12 + 4;
    const aux = position + ifdLength;
    const bitsAt = aux;
    const xresAt = aux + 8;
    const yresAt = aux + 16;
    const stripAt = aux + 24;
    const stripLength = width * height * 4;
    const padding = stripLength % 2;
    const next = index === reps.length - 1 ? 0 : stripAt + stripLength + padding;

    const ifd = Buffer.alloc(ifdLength);
    ifd.writeUInt16LE(tags, 0);
    let at = 2;
    const entry = (tag, type, count, value) => {
      ifd.writeUInt16LE(tag, at);
      ifd.writeUInt16LE(type, at + 2);
      ifd.writeUInt32LE(count, at + 4);
      if (type === SHORT && count === 1) ifd.writeUInt16LE(value, at + 8);
      else ifd.writeUInt32LE(value, at + 8);
      at += 12;
    };
    entry(256, LONG, 1, width);
    entry(257, LONG, 1, height);
    entry(258, SHORT, 4, bitsAt);
    entry(259, SHORT, 1, 1); // uncompressed
    entry(262, SHORT, 1, 2); // RGB
    entry(273, LONG, 1, stripAt);
    entry(277, SHORT, 1, 4); // samples per pixel
    entry(278, LONG, 1, height); // rows per strip
    entry(279, LONG, 1, stripLength);
    entry(282, RATIONAL, 1, xresAt);
    entry(283, RATIONAL, 1, yresAt);
    entry(284, SHORT, 1, 1); // chunky
    entry(296, SHORT, 1, 2); // inches
    entry(338, SHORT, 1, 1); // associated alpha
    ifd.writeUInt32LE(next, at);

    const extra = Buffer.alloc(24);
    [8, 8, 8, 8].forEach((bits, i) => extra.writeUInt16LE(bits, i * 2));
    extra.writeUInt32LE(dpi, 8);
    extra.writeUInt32LE(1, 12);
    extra.writeUInt32LE(dpi, 16);
    extra.writeUInt32LE(1, 20);

    chunks.push(ifd, extra, Buffer.from(pixels), Buffer.alloc(padding));
    position = stripAt + stripLength + padding;
  });
  return Buffer.concat(chunks);
}

// ---------------------------------------------------------------------------
// Emitting
// ---------------------------------------------------------------------------

const written = [];

/** Writes a file only when its bytes change, so a re-run is a no-op. */
function emit(path, data) {
  const bytes = Buffer.isBuffer(data) ? data : Buffer.from(data, "utf8");
  if (existsSync(path) && readFileSync(path).equals(bytes)) return;
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, bytes);
  written.push(path);
}

const json = (value) => `${JSON.stringify(value, null, 2)}\n`;

// ---------------------------------------------------------------------------
// The outputs
// ---------------------------------------------------------------------------

function renderMacos(marks) {
  const forSize = (px) => macosTile(marks.at(px), { shadow: px >= 64 });
  const entries = ICNS_TYPES.map(([, px, type]) => ({ type, data: png(forSize(px), px) }));
  emit(join(OUT, "macos", "AppIcon.icns"), icns(entries));
  // Full size PNGs for anything that wants one — the Dock in development,
  // the README.
  emit(join(OUT, "macos", "AppIcon-1024.png"), png(forSize(1024), 1024));
  emit(join(OUT, "macos", "AppIcon-1024-dark.png"), png(macosTile(marks.at(1024), { dark: true }), 1024));
  renderIconComposer(marks);
}

/**
 * The macOS 26 icon as Icon Composer's document: a fill and one glyph
 * layer, from which the system derives the light, dark, clear and tinted
 * renditions itself. The schema follows the packages Icon Composer writes.
 */
function renderIconComposer(marks) {
  const pkg = join(OUT, "macos", "EverTranscript.icon");
  const rgb = (hex) => {
    const n = parseInt(hex.slice(1), 16);
    const c = (v) => (v / 255).toFixed(5);
    return `srgb:${c(n >> 16)},${c((n >> 8) & 255)},${c(n & 255)},1.00000`;
  };
  // The glyph as its own document, drawn at the size it should appear on
  // the 1024-point canvas, so the layer imports at scale 1.
  // Filled outlines, not strokes: Icon Composer fills whatever path it is
  // given and ignores the stroke, which turned a stroked mark into a disc.
  const glyphDocument =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">` +
    place(outlineGlyph(marks.at(1024)), { cx: 512, cy: 512, inkWidth: 1024 * GLYPH_FRACTION, color: COLOR.paper }) +
    `</svg>`;
  emit(join(pkg, "Assets", "Mark.svg"), glyphDocument);
  emit(
    join(pkg, "icon.json"),
    json({
      fill: {
        "linear-gradient": [rgb(COLOR.teal400), rgb(COLOR.teal700)],
        orientation: { start: { x: 0.5, y: 0 }, stop: { x: 0.5, y: 1 } },
      },
      groups: [
        {
          layers: [
            {
              "fill-specializations": [
                { value: { solid: rgb(COLOR.paper) } },
                { appearance: "dark", value: { solid: rgb(COLOR.paper) } },
              ],
              "image-name": "Mark.svg",
              name: "Mark",
              position: { scale: 1, "translation-in-points": [0, 0] },
            },
          ],
          shadow: { kind: "neutral", opacity: 0.5 },
          translucency: { enabled: true, value: 0.5 },
        },
      ],
      "supported-platforms": { circles: ["watchOS"], squares: "shared" },
    }),
  );
}

function renderIos(marks) {
  const set = join(OUT, "ios", "Assets.xcassets", "AppIcon.appiconset");
  const glyph = marks.at(1024);
  // No alpha channel: App Store Connect rejects a 1024 icon that has one.
  emit(join(set, "AppIcon-1024.png"), opaquePng(rasterize(squareTile(glyph), 1024), COLOR.teal500));
  // Dark: the system supplies the dark background; only the mark is drawn.
  emit(join(set, "AppIcon-1024-dark.png"), png(squareTile(glyph, { background: false }), 1024));
  // Tinted: a grayscale image the system colours. Lighter means more tint.
  emit(join(set, "AppIcon-1024-tinted.png"), png(squareTile(glyph, { background: false, glyphColor: "#E6E6E6" }), 1024));
  const entry = (filename, appearance) => ({
    ...(appearance ? { appearances: [{ appearance: "luminosity", value: appearance }] } : {}),
    filename,
    idiom: "universal",
    platform: "ios",
    size: "1024x1024",
  });
  emit(
    join(set, "Contents.json"),
    json({
      images: [entry("AppIcon-1024.png"), entry("AppIcon-1024-dark.png", "dark"), entry("AppIcon-1024-tinted.png", "tinted")],
      info: { author: "xcode", version: 1 },
    }),
  );
  emit(join(OUT, "ios", "Assets.xcassets", "Contents.json"), json({ info: { author: "xcode", version: 1 } }));
}

function renderAndroid(marks) {
  const res = join(OUT, "android", "res");
  const densities = [
    ["mdpi", 1],
    ["hdpi", 1.5],
    ["xhdpi", 2],
    ["xxhdpi", 3],
    ["xxxhdpi", 4],
  ];
  // Adaptive icon foreground: a 108 dp canvas whose outer 21 dp the
  // launcher may mask away; the mark stays inside the 66 dp safe circle.
  const foreground = (glyph) =>
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 108 108">` +
    place(glyph, { cx: 54, cy: 54, inkWidth: 44, color: COLOR.paper }) +
    `</svg>`;
  for (const [density, scale] of densities) {
    const dir = join(res, `mipmap-${density}`);
    const fg = Math.round(108 * scale);
    const legacy = Math.round(48 * scale);
    emit(join(dir, "ic_launcher_foreground.png"), png(foreground(marks.at(fg)), fg));
    emit(join(dir, "ic_launcher.png"), png(roundedTile(marks.at(legacy)), legacy));
    emit(join(dir, "ic_launcher_round.png"), png(circleTile(marks.at(legacy)), legacy));
  }
  const adaptive =
    `<?xml version="1.0" encoding="utf-8"?>\n` +
    `<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">\n` +
    `    <background android:drawable="@color/ic_launcher_background"/>\n` +
    `    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>\n` +
    `    <monochrome android:drawable="@mipmap/ic_launcher_foreground"/>\n` +
    `</adaptive-icon>\n`;
  emit(join(res, "mipmap-anydpi-v26", "ic_launcher.xml"), adaptive);
  emit(join(res, "mipmap-anydpi-v26", "ic_launcher_round.xml"), adaptive);
  emit(
    join(res, "values", "ic_launcher_background.xml"),
    `<?xml version="1.0" encoding="utf-8"?>\n<resources>\n    <color name="ic_launcher_background">${COLOR.teal500}</color>\n</resources>\n`,
  );
  emit(join(OUT, "android", "playstore-512.png"), opaquePng(rasterize(squareTile(marks.at(512)), 512), COLOR.teal500));
}

function renderWindows(marks) {
  const sizes = [16, 24, 32, 48, 64, 128, 256];
  const images = sizes.map((size) => ({ size, data: png(roundedTile(marks.at(size)), size) }));
  emit(join(OUT, "windows", "EverTranscript.ico"), ico(images));
  emit(join(OUT, "windows", "EverTranscript-256.png"), images[images.length - 1].data);
}

function renderWeb(marks) {
  const web = join(OUT, "web");
  // The SVG favicon is the one file that stays vector: the small mark on
  // the rounded tile, at any size the browser wants.
  emit(join(web, "favicon.svg"), `${roundedTile(marks.at(16), { size: 64 })}\n`);
  emit(join(web, "favicon.ico"), ico([16, 32, 48].map((size) => ({ size, data: png(roundedTile(marks.at(size)), size) }))));
  emit(join(web, "favicon-32.png"), png(roundedTile(marks.at(32)), 32));
  emit(join(web, "apple-touch-icon.png"), opaquePng(rasterize(squareTile(marks.at(180)), 180), COLOR.teal500));
  emit(join(web, "icon-192.png"), png(roundedTile(marks.at(192)), 192));
  emit(join(web, "icon-512.png"), png(roundedTile(marks.at(512)), 512));
  // Maskable: full bleed, the mark inside the 80% safe zone.
  emit(join(web, "maskable-512.png"), png(squareTile(marks.at(512), { glyphFraction: GLYPH_FRACTION * 0.8 }), 512));
  emit(
    join(web, "site.webmanifest"),
    json({
      name: "EverTranscript",
      short_name: "EverTranscript",
      description: "A live and archived transcript of every meeting.",
      icons: [
        { src: "/icon-192.png", sizes: "192x192", type: "image/png" },
        { src: "/icon-512.png", sizes: "512x512", type: "image/png" },
        { src: "/maskable-512.png", sizes: "512x512", type: "image/png", purpose: "maskable" },
      ],
      theme_color: COLOR.teal500,
      background_color: COLOR.paper,
      display: "standalone",
    }),
  );
}

/** The menu bar's four states, as the TIFFs `tray/macos.rs` embeds. */
function renderTray(marks) {
  const glyphs = join(REPO, "crates", "evertranscript-core", "src", "tray", "glyphs");
  for (const state of ["ready", "recording", "busy", "attention"]) {
    const svg = trayGlyph(marks.at(18), { state });
    const x1 = rasterize(svg, 18);
    const x2 = rasterize(svg, 36);
    emit(
      join(glyphs, `${state}.tiff`),
      tiff([
        { width: x1.width, height: x1.height, pixels: x1.pixels, dpi: 72 },
        { width: x2.width, height: x2.height, pixels: x2.pixels, dpi: 144 },
      ]),
    );
    // Previews for eyes: the vector, and the 2x raster on both bar colours.
    emit(join(OUT, "tray", `${state}.svg`), `${svg}\n`);
    emit(join(OUT, "tray", `${state}@2x.png`), x2.asPng());
    emit(join(OUT, "tray", `${state}-dark@2x.png`), png(trayGlyph(marks.at(18), { state, color: "#FFFFFF" }), 36));
  }
}

/** The name, as set in the lockups. */
const WORDMARK_TEXT = "EverTranscript";
// The TrueType build: opentype.js drops glyphs from the CFF (.otf) one.
const WORDMARK_FONT = join(BRAND, "fonts", "Geist-SemiBold.ttf");

/**
 * The name set in Geist SemiBold and converted to outlines, so the lockups
 * are plain paths and no consumer ever needs the font. Geist is OFL; the
 * licence sits beside the file in `fonts/`.
 */
function wordmark() {
  const bytes = readFileSync(WORDMARK_FONT);
  const font = opentype.parse(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
  const size = 100;
  // Tight, not touching: a name this long wants the letters to hold together.
  const path = font.getPath(WORDMARK_TEXT, 0, 0, size, { kerning: true, letterSpacing: -0.018 });
  const box = path.getBoundingBox();
  const capHeight = ((font.tables.os2?.sCapHeight ?? font.ascender * 0.7) / font.unitsPerEm) * size;
  // Serialized by hand: opentype.js's own toPathData appends a command with
  // NaN in it, and one non-finite number keeps the renderer from drawing
  // any of the path.
  const f = (n) => {
    if (!Number.isFinite(n)) throw new Error("wordmark: non-finite coordinate");
    return n.toFixed(2).replace(/\.?0+$/, "");
  };
  const d = path.commands
    .map((c) => {
      switch (c.type) {
        case "M":
          return `M${f(c.x)} ${f(c.y)}`;
        case "L":
          return `L${f(c.x)} ${f(c.y)}`;
        case "Q":
          return `Q${f(c.x1)} ${f(c.y1)} ${f(c.x)} ${f(c.y)}`;
        case "C":
          return `C${f(c.x1)} ${f(c.y1)} ${f(c.x2)} ${f(c.y2)} ${f(c.x)} ${f(c.y)}`;
        case "Z":
          return "Z";
        default:
          throw new Error(`wordmark: unexpected command ${c.type}`);
      }
    })
    .join("");
  return { d, box, capHeight };
}

/** The wordmark on its own and beside the mark, in ink and in paper. */
function renderLockups(marks) {
  const { d, box, capHeight } = wordmark();
  const pad = capHeight * 0.3;
  const f = (n) => n.toFixed(2);
  const document = (left, top, right, bottom, body) =>
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${f(left)} ${f(top)} ${f(right - left)} ${f(bottom - top)}">${body}</svg>\n`;

  const word = (color) =>
    document(box.x1 - pad, box.y1 - pad, box.x2 + pad, box.y2 + pad, `<path d="${d}" fill="${color}"/>`);
  emit(join(OUT, "lockups", "wordmark.svg"), word(COLOR.ink));
  emit(join(OUT, "lockups", "wordmark-dark.svg"), word(COLOR.paper));

  // The mark sits to the left, a little taller than the capitals and
  // centred on them; the baseline is y = 0, so the capitals span -capHeight..0.
  const inkWidth = capHeight * 1.2;
  const gap = capHeight * 0.45;
  const cx = box.x1 - gap - inkWidth / 2;
  const cy = -capHeight / 2;
  const top = Math.min(box.y1, cy - inkWidth / 2) - pad;
  const bottom = Math.max(box.y2, cy + inkWidth / 2) + pad;
  const lockup = (color) =>
    document(
      cx - inkWidth / 2 - pad,
      top,
      box.x2 + pad,
      bottom,
      place(marks.mark, { cx, cy, inkWidth, color }) + `<path d="${d}" fill="${color}"/>`,
    );
  emit(join(OUT, "lockups", "lockup-light.svg"), lockup(COLOR.ink));
  emit(join(OUT, "lockups", "lockup-dark.svg"), lockup(COLOR.paper));

  // Previews on their grounds, for the README and for eyes.
  emit(join(OUT, "lockups", "lockup-light@2x.png"), rasterize(lockup(COLOR.ink), 1600, { background: COLOR.paper }).asPng());
  emit(join(OUT, "lockups", "lockup-dark@2x.png"), rasterize(lockup(COLOR.paper), 1600, { background: COLOR.ink }).asPng());
  emit(join(OUT, "lockups", "mark.svg"), `${readFileSync(join(SRC, "mark.svg"), "utf8").trim()}\n`);
  // The same drawing as filled outlines, for anything that cannot stroke.
  emit(
    join(OUT, "lockups", "mark-outlined.svg"),
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256"><g color="${COLOR.ink}">${outlineGlyph(marks.mark)}</g></svg>\n`,
  );
}

/** The Electron Client's copies, flat, where `src/main/index.ts` looks. */
function renderElectron() {
  const resources = join(REPO, "clients", "electron", "resources");
  emit(join(resources, "icon.icns"), readFileSync(join(OUT, "macos", "AppIcon.icns")));
  emit(join(resources, "icon.ico"), readFileSync(join(OUT, "windows", "EverTranscript.ico")));
  emit(join(resources, "icon.png"), readFileSync(join(OUT, "macos", "AppIcon-1024.png")));
}

/** The mark, with its small-size variant chosen by output size. */
function loadMarks(markPath, smallPath) {
  const mark = loadGlyph(markPath);
  const small = smallPath && existsSync(smallPath) ? loadGlyph(smallPath) : mark;
  return { at: (px) => (px <= 32 ? small : mark), mark, small };
}

function renderAll() {
  const marks = loadMarks(join(SRC, "mark.svg"), join(SRC, "mark-small.svg"));
  renderMacos(marks);
  renderIos(marks);
  renderAndroid(marks);
  renderWindows(marks);
  renderWeb(marks);
  renderTray(marks);
  renderLockups(marks);
  renderElectron();
}

// ---------------------------------------------------------------------------
// Explorations: the concept contact sheet
// ---------------------------------------------------------------------------

/**
 * Lays out rows of pre-rendered images on one canvas. Each cell is drawn at
 * an integer `zoom` with nearest-neighbour sampling, so what is shown is
 * exactly the pixels a screen would get.
 */
function sheet(rows, { gap = 24, pad = 32, background = "#FFFFFF" } = {}) {
  let y = pad;
  let widest = 0;
  const items = [];
  for (const row of rows) {
    let x = pad;
    let tallest = 0;
    for (const cell of row) {
      const w = cell.width * (cell.zoom ?? 1);
      const h = cell.height * (cell.zoom ?? 1);
      if (cell.backdrop) {
        items.push(
          `<rect x="${x - cell.backdrop.pad}" y="${y - cell.backdrop.pad}" width="${w + 2 * cell.backdrop.pad}" ` +
            `height="${h + 2 * cell.backdrop.pad}" rx="${cell.backdrop.radius ?? 0}" fill="${cell.backdrop.fill}"/>`,
        );
      }
      const href = `data:image/png;base64,${cell.png.toString("base64")}`;
      items.push(`<image x="${x}" y="${y}" width="${w}" height="${h}" image-rendering="optimizeSpeed" href="${href}"/>`);
      x += w + gap + (cell.backdrop ? 2 * cell.backdrop.pad : 0);
      tallest = Math.max(tallest, h + (cell.backdrop ? 2 * cell.backdrop.pad : 0));
    }
    widest = Math.max(widest, x - gap + pad);
    y += tallest + gap * 2;
  }
  const height = y - gap * 2 + pad;
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${widest}" height="${height}" viewBox="0 0 ${widest} ${height}">` +
    `<rect width="${widest}" height="${height}" fill="${background}"/>${items.join("")}</svg>`;
  return rasterize(svg, widest, { imageRendering: 1 }).asPng();
}

const cell = (buffer, width, height, zoom = 1, backdrop) => ({ png: buffer, width, height, zoom, backdrop });

/**
 * Stand-ins for Dock neighbours: the colours of the apps next door, with a
 * letter or a stroke where their marks go. Not their icons — those are
 * theirs — just enough to see the teal against the blues and the green.
 */
function neighbour(fill, shape) {
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">` +
    `<rect x="100" y="100" width="824" height="824" rx="185" fill="${fill}"/>` +
    `<g transform="translate(512 512) scale(3.2) translate(-128 -128)">${shape}</g>` +
    `</svg>`;
  return png(svg, 64);
}
const NEIGHBOURS = [
  ["#2D8CFF", `<path d="M64,80 L192,80 L192,112 L120,176 L192,176 L192,208 L64,208 L64,176 L136,112 L64,112 Z" fill="#FFFFFF"/>`],
  ["#5B5FC7", `<path d="M48,80 L208,80 L208,112 L144,112 L144,208 L112,208 L112,112 L48,112 Z" fill="#FFFFFF"/>`],
  [
    "#B2C248",
    `<path d="M128,56 A72,72 0 1 1 56,128 A56,56 0 1 1 168,128 A40,40 0 1 1 88,128 A24,24 0 1 1 136,128 A8,8 0 1 1 120,128" ` +
      `fill="none" stroke="#1F1D1B" stroke-width="18" stroke-linecap="round"/>`,
  ],
];

function renderExplorations(outDir) {
  const dir = join(BRAND, "explorations");
  const concepts = readdirSync(dir)
    .filter((name) => name.endsWith(".svg"))
    .sort();
  const rows = [];
  const light = "#ECECEC";
  const dark = "#1E1E1E";
  const bar = { pad: 6, radius: 4 };
  for (const name of concepts) {
    const marks = loadMarks(join(dir, name));
    const slug = basename(name, ".svg");
    const at = (px) => png(macosTile(marks.at(px), { shadow: px >= 64 }), px);
    const tray = (color, px) => png(trayGlyph(marks.at(18), { color }), px);
    const files = {
      256: at(256),
      128: at(128),
      64: at(64),
      32: at(32),
      16: at(16),
      "tray@1x": tray("#000000", 18),
      "tray@2x": tray("#000000", 36),
      "tray-dark@2x": tray("#FFFFFF", 36),
      favicon: png(roundedTile(marks.at(16)), 16),
    };
    for (const [key, buffer] of Object.entries(files)) emit(join(outDir, `${slug}-${key}.png`), buffer);
    rows.push([
      cell(files[256], 256, 256),
      cell(files[128], 128, 128),
      cell(files[64], 64, 64),
      cell(files[32], 32, 32),
      cell(files[16], 16, 16),
      cell(files[32], 32, 32, 4),
      cell(files[16], 16, 16, 6),
      cell(files["tray@1x"], 18, 18, 1, { ...bar, fill: light }),
      cell(files["tray@2x"], 36, 36, 1, { ...bar, fill: light }),
      cell(files["tray@2x"], 36, 36, 3, { ...bar, fill: light }),
      cell(files["tray-dark@2x"], 36, 36, 1, { ...bar, fill: dark }),
      cell(files["tray-dark@2x"], 36, 36, 3, { ...bar, fill: dark }),
      cell(files.favicon, 16, 16, 1, { pad: 8, radius: 6, fill: "#F3F3F3" }),
      cell(files.favicon, 16, 16, 4, { pad: 8, radius: 6, fill: "#F3F3F3" }),
      ...NEIGHBOURS.map(([fill, letter]) => cell(neighbour(fill, letter), 64, 64)),
      cell(files[64], 64, 64),
    ]);
  }
  const png_ = sheet(rows, { background: "#FAFAF8" });
  emit(join(dir, "contact-sheet.png"), png_);
  emit(join(outDir, "contact-sheet.png"), png_);
}

// ---------------------------------------------------------------------------

const args = process.argv.slice(2);
if (args[0] === "--explorations") {
  renderExplorations(args[1] ?? join(BRAND, "explorations", "render"));
} else {
  renderAll();
}
if (written.length === 0) console.log("nothing changed");
else for (const path of written) console.log(`wrote ${path.replace(`${REPO}/`, "")}`);
