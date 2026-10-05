// Where the sun is, and where it's day and night, for the world map (equirectangular:
// x = longitude + 180, y = 90 - latitude). Accurate to a fraction of a degree, plenty
// for a map: the low-precision formulas of the Astronomical Almanac.

const RAD = Math.PI / 180;

/** The point the sun is overhead at `date`: { lat, lon } in degrees. */
export function subsolar(date = new Date()) {
  const n = date.getTime() / 86400000 - 10957.5; // days since J2000 (2000-01-01 12:00 UTC)
  const L = 280.46 + 0.9856474 * n;
  const g = (357.528 + 0.9856003 * n) * RAD;
  const lambda = (L + 1.915 * Math.sin(g) + 0.02 * Math.sin(2 * g)) * RAD;
  const epsilon = (23.439 - 0.0000004 * n) * RAD;
  const decl = Math.asin(Math.sin(epsilon) * Math.sin(lambda));
  const ra = Math.atan2(Math.cos(epsilon) * Math.sin(lambda), Math.cos(lambda));
  const gmst = 280.46061837 + 360.98564736629 * n;
  const lon = ((((ra / RAD - gmst) % 360) + 540) % 360) - 180;
  return { lat: decl / RAD, lon };
}

/** The sun's height above the horizon (degrees) at a place. */
export function elevation(lat, lon, sun) {
  const h = (lon - sun.lon) * RAD;
  const s = Math.sin(lat * RAD) * Math.sin(sun.lat * RAD) + Math.cos(lat * RAD) * Math.cos(sun.lat * RAD) * Math.cos(h);
  return Math.asin(Math.max(-1, Math.min(1, s))) / RAD;
}

/** Where along the meridian at `lon` the sun is below `below` degrees: the dark runs, as
 * [top, bottom] latitudes, the crossings found between samples a degree apart. The
 * twilights' edges are circles around the point opposite the sun, which a meridian can
 * cross twice, so there may be two runs. */
function darkRuns(sun, below, lon) {
  const runs = [];
  let start = null;
  let prev = null;
  for (let lat = 90; lat >= -90; lat -= 1) {
    const e = elevation(lat, lon, sun) - below;
    if (prev === null) {
      if (e < 0) start = 90;
    } else {
      const [plat, pe] = prev;
      const cross = pe === e ? lat : plat + (pe / (pe - e)) * (lat - plat);
      if (pe >= 0 && e < 0) start = cross;
      if (pe < 0 && e >= 0 && start !== null) {
        runs.push([start, cross]);
        start = null;
      }
    }
    prev = [lat, e];
  }
  if (start !== null) runs.push([start, -90]);
  return runs;
}

/** An SVG path covering where the sun is below `below` degrees (0: night; -6, -12, -18:
 * the twilights' ends), on the map's coordinates: a strip per `step` degrees of longitude
 * whose top and bottom follow the edge from one side of the strip to the other (a plain
 * rectangle where the two sides don't match, at a run's tip). */
export function darkPath(sun, below = 0, step = 2) {
  const parts = [];
  let left = darkRuns(sun, below, -180);
  for (let lon = -180; lon < 180; lon += step) {
    const right = darkRuns(sun, below, lon + step);
    const x0 = lon + 180;
    // A hair wider than the step, so neighbouring strips leave no seam.
    const x1 = x0 + step + 0.05;
    const y = lat => (90 - lat).toFixed(2);
    if (left.length === right.length) {
      left.forEach(([t0, b0], i) => {
        const [t1, b1] = right[i];
        parts.push(`M${x0},${y(t0)}L${x1},${y(t1)}L${x1},${y(b1)}L${x0},${y(b0)}Z`);
      });
    } else {
      for (const [t, b] of darkRuns(sun, below, lon + step / 2)) parts.push(`M${x0},${y(t)}L${x1},${y(t)}L${x1},${y(b)}L${x0},${y(b)}Z`);
    }
    left = right;
  }
  return parts.join('');
}

/** The local solar time at a longitude, as HH:MM (what a clock there roughly shows). */
export function solarTime(lon, date = new Date()) {
  const minutes = ((date.getUTCHours() * 60 + date.getUTCMinutes() + lon * 4) % 1440 + 1440) % 1440;
  return `${String(Math.floor(minutes / 60)).padStart(2, '0')}:${String(Math.floor(minutes % 60)).padStart(2, '0')}`;
}
