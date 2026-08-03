// Burn-down over time, drawn as an inline SVG sparkline.
//
// No charting library: this is one line over a fixed 0-100 range. Axes, zoom and legends
// would be a dependency bought to draw a path element.

import { useEffect, useState } from "react";
import { api, severity, type Reading } from "./api";

export function History() {
  const [readings, setReadings] = useState<Reading[]>([]);

  useEffect(() => {
    api.history().then(setReadings);
  }, []);

  if (readings.length === 0) {
    return (
      <div className="empty">
        <p>No history yet.</p>
        <p className="muted">
          A reading is recorded on every refresh — leave axio quota running and
          this fills in.
        </p>
      </div>
    );
  }

  // Series keyed by provider and window, in first-seen order so the charts appear in the
  // same order as the Providers view.
  //
  // The pair is kept as a pair. Joining it into one string and splitting on a space —
  // which is what this did — silently truncated every label containing one: "Weekly
  // (Fable)" became a second "Weekly" and drew a duplicate chart, and
  // "GPT-5.3-Codex-Spark Weekly" matched no reading at all and vanished.
  const bySeries = new Map<string, Reading[]>();
  for (const reading of readings) {
    const key = JSON.stringify([reading.provider, reading.label]);
    const existing = bySeries.get(key);
    if (existing) existing.push(reading);
    else bySeries.set(key, [reading]);
  }

  return (
    <>
      <p className="muted">{readings.length} readings</p>
      {[...bySeries.entries()].map(([key, series]) => {
        const { provider, label } = series[0];
        // A single point is a dot, not a trend; drawing it invites reading meaning into
        // one sample.
        if (series.length < 2) return null;
        const latest = series[series.length - 1].used_percent;
        return (
          <div className="series" key={key}>
            <div className="series-head">
              <span className="series-name">
                {provider} <span className="muted">— {label}</span>
              </span>
              <span className={`series-now ${severity(latest)}`}>
                {Math.round(latest)}%
              </span>
            </div>
            <Chart series={series} />
            <div className="series-span">
              <span>{when(series[0].at)}</span>
              <span>{when(series[series.length - 1].at)}</span>
            </div>
          </div>
        );
      })}
    </>
  );
}

/// A reading's date, short enough to sit under a sparkline.
function when(at: string): string {
  return new Date(at).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

/// A sparkline, coloured by the same rule the rails use.
///
/// Amber is the product accent and rule 1 of this sheet reserves it for identity: it draws
/// a 2px edge or a 4px mark and never a fill. A line that states consumption belongs to
/// the signal family instead, so it takes the ok/warn/crit ramp and agrees with the rail
/// on the Providers tab for the same number — which is the point of having one ramp.
function Chart({ series }: { series: Reading[] }) {
  const width = 600;
  const height = 100;
  const first = new Date(series[0].at).getTime();
  const last = new Date(series[series.length - 1].at).getTime();
  const span = Math.max(1, last - first);

  const point = (reading: Reading) => {
    const x = ((new Date(reading.at).getTime() - first) / span) * width;
    // Fixed 0-100 rather than fitted: an auto-scaled axis makes a flat 2% week look
    // identical to a flat 90% one. The area fill is what keeps a genuinely low line
    // legible as "barely touched" rather than as an empty box with a rule at the bottom.
    const y = height - (Math.min(100, reading.used_percent) / 100) * height;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  };

  const points = series.map(point).join(" ");
  const tone = severity(series[series.length - 1].used_percent);

  return (
    <svg
      className={`chart ${tone}`}
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      {/* Quarters of the window, so the height a line sits at can be read without a
          scale. The 100% mark is the top edge of the box itself. */}
      {[0.25, 0.5, 0.75].map((fraction) => (
        <line
          key={fraction}
          x1={0}
          x2={width}
          y1={height * fraction}
          y2={height * fraction}
          stroke="#262626"
          strokeWidth={1}
          vectorEffect="non-scaling-stroke"
        />
      ))}
      <polygon
        points={`0,${height} ${points} ${width},${height}`}
        fill="currentColor"
        fillOpacity={0.13}
      />
      <polyline
        points={points}
        fill="none"
        stroke="currentColor"
        strokeWidth={1.6}
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}
