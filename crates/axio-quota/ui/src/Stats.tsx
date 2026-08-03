// The year, as a calendar.
//
// The tables answer "what did I spend". This answers "when do I actually work", which no
// grouping of a table reaches: a run of days is a shape, and a shape has to be drawn.

import { useEffect, useState } from "react";
import { api, type DayPoint, type Stats as StatsData } from "./api";

const WEEKS = 53;
const DAYS = 7;

/// Bucket a day's tokens into one of four intensities.
///
/// The cuts are quartiles of the active days and come from Rust, so the window and
/// `axio cost --calendar` shade the same day the same way. Scaling against the busiest day
/// instead — linearly or logarithmically — draws one flat colour on a year whose peak is
/// thirty times its median; see `Stats::thresholds` for the measurement.
function intensity(tokens: number, cuts: [number, number, number]): number {
  if (tokens <= 0) return 0;
  return 1 + cuts.filter((cut) => tokens >= cut).length;
}

/// The Sunday on or before a date, in UTC.
function weekStart(date: Date): Date {
  const start = new Date(date);
  start.setUTCDate(start.getUTCDate() - start.getUTCDay());
  start.setUTCHours(0, 0, 0, 0);
  return start;
}

function iso(date: Date): string {
  return date.toISOString().slice(0, 10);
}

const MONTHS = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

export function Stats() {
  const [stats, setStats] = useState<StatsData | null>(null);

  useEffect(() => {
    api.costStats().then(setStats);
  }, []);

  if (!stats) {
    return (
      <>
        <h3>Reading session transcripts…</h3>
        <p className="muted">
          The first scan walks every agent's logs on this machine.
        </p>
      </>
    );
  }

  if (stats.days.length === 0) {
    return <p className="empty">No sessions found on this machine yet.</p>;
  }

  const byDate = new Map<string, DayPoint>(stats.days.map((day) => [day.date, day]));

  // The grid ends on the current week and runs back a year, so "now" is always the last
  // column. Anchoring on the newest *data* instead would make a quiet fortnight look like
  // the calendar had stopped.
  const lastWeek = weekStart(new Date());
  const columns: { date: string; day: DayPoint | undefined }[][] = [];
  for (let week = WEEKS - 1; week >= 0; week -= 1) {
    const column: { date: string; day: DayPoint | undefined }[] = [];
    for (let weekday = 0; weekday < DAYS; weekday += 1) {
      const cell = new Date(lastWeek);
      cell.setUTCDate(cell.getUTCDate() - week * 7 + weekday);
      const key = iso(cell);
      column.push({ date: key, day: byDate.get(key) });
    }
    columns.push(column);
  }

  // A month label sits over the first column that belongs to it, which is how a reader
  // finds July without counting squares.
  const monthLabels = columns.map((column, index) => {
    const first = new Date(`${column[0].date}T00:00:00Z`);
    if (index === 0) return MONTHS[first.getUTCMonth()];
    const previous = new Date(`${columns[index - 1][0].date}T00:00:00Z`);
    return first.getUTCMonth() === previous.getUTCMonth()
      ? ""
      : MONTHS[first.getUTCMonth()];
  });

  return (
    <>
      <div className="heatmap" role="img" aria-label={`Usage over the last year: ${stats.activeDays} active days`}>
        <div className="heatmap-months">
          {monthLabels.map((label, index) => (
            <span key={index}>{label}</span>
          ))}
        </div>
        <div className="heatmap-grid">
          <div className="heatmap-days">
            <span>Mon</span>
            <span>Wed</span>
            <span>Fri</span>
          </div>
          <div className="heatmap-cells">
            {columns.map((column, index) => (
              <div className="heatmap-week" key={index}>
                {column.map(({ date, day }) => (
                  <i
                    key={date}
                    className={`cell level-${day ? intensity(day.tokens, stats.thresholds) : 0}`}
                    title={
                      day
                        ? `${date} · ${day.tokens.toLocaleString()} tokens · ${day.messages} messages${day.costUsd !== null ? ` · $${day.costUsd.toFixed(2)}` : ""}`
                        : `${date} · nothing`
                    }
                  />
                ))}
              </div>
            ))}
          </div>
        </div>
        <div className="heatmap-key">
          <span className="muted">Less</span>
          {[0, 1, 2, 3, 4].map((level) => (
            <i key={level} className={`cell level-${level}`} />
          ))}
          <span className="muted">More</span>
        </div>
      </div>

      <dl className="figures">
        <Figure label="Total cost" value={stats.totalCostUsd !== null ? `$${stats.totalCostUsd.toFixed(2)}` : "unpriced"} />
        <Figure label="Total tokens" value={stats.totalTokens.toLocaleString()} />
        <Figure label="Sessions" value={stats.sessions.toLocaleString()} />
        <Figure label="Active days" value={`${stats.activeDays}`} />
        <Figure label="Current streak" value={`${stats.currentStreak} ${stats.currentStreak === 1 ? "day" : "days"}`} />
        <Figure label="Longest streak" value={`${stats.longestStreak} ${stats.longestStreak === 1 ? "day" : "days"}`} />
        <Figure label="Most used" value={stats.topModel ?? "—"} />
      </dl>
    </>
  );
}

function Figure({ label, value }: { label: string; value: string }) {
  return (
    <div className="figure">
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}
