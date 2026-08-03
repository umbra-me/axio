// The year, as a calendar.
//
// The tables answer "what did I spend". This answers "when do I actually work", which no
// grouping of a table reaches: a run of days is a shape, and a shape has to be drawn.

import { useEffect, useState } from "react";
import { api, onCostUpdated, type CostRow, type DayPoint, type Stats as StatsData } from "./api";

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

/// A big count, short enough to fit a card.
///
/// 18,374,235,696 does not fit in a 150px column and was being ellipsed to
/// "18,374,235,6…", which is worse than useless — it reads as a smaller number. The exact
/// figure stays in the card's tooltip; the table is where someone goes for digits.
function compact(value: number): string {
  if (value < 1_000) return `${value}`;
  const units = ["K", "M", "B", "T"];
  let scaled = value;
  let unit = -1;
  while (scaled >= 1_000 && unit < units.length - 1) {
    scaled /= 1_000;
    unit += 1;
  }
  return `${scaled.toFixed(scaled < 10 ? 2 : 1)}${units[unit]}`;
}

const MONTHS = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

export function Stats() {
  const [stats, setStats] = useState<StatsData | null>(null);

  useEffect(() => {
    const load = () => api.costStats().then(setStats);
    load();
    // The saved scan publishes first and the live one replaces it a moment later.
    const unlisten = onCostUpdated(load);
    return () => {
      unlisten.then((off) => off());
    };
  }, []);

  if (!stats) {
    return (
      <>
        <h3>Reading session transcripts…</h3>
        <p className="muted">
          The first scan walks every agent's logs on this machine. The result is saved, so
          later launches draw this straight away.
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

  const busiest = stats.days.reduce<DayPoint | null>(
    (best, day) => (best === null || day.tokens > best.tokens ? day : best),
    null,
  );

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
        <Figure
          label="Total tokens"
          value={compact(stats.totalTokens)}
          exact={stats.totalTokens.toLocaleString()}
        />
        <Figure label="Sessions" value={stats.sessions.toLocaleString()} />
        <Figure label="Active days" value={`${stats.activeDays}`} />
        <Figure label="Current streak" value={`${stats.currentStreak} ${stats.currentStreak === 1 ? "day" : "days"}`} />
        <Figure label="Longest streak" value={`${stats.longestStreak} ${stats.longestStreak === 1 ? "day" : "days"}`} />
        <Figure label="Most used" value={stats.topModel ?? "—"} />
        {busiest && (
          <Figure
            label="Busiest day"
            value={busiest.date}
            exact={`${busiest.tokens.toLocaleString()} tokens`}
          />
        )}
        <Figure
          label="Cost per active day"
          value={
            stats.totalCostUsd !== null && stats.activeDays > 0
              ? `$${(stats.totalCostUsd / stats.activeDays).toFixed(2)}`
              : "—"
          }
        />
      </dl>

      <Section
        title="Daily spend"
        note="The last 60 days. Bars are cost, so a cheap busy day and an expensive quiet one read differently here than on the calendar."
      >
        <Spend days={stats.days.slice(-60)} />
      </Section>

      <div className="pair">
        <Section title="By hour" note="UTC, not local — see the tooltip.">
          <Histogram
            values={stats.byHour}
            label={(index) => `${String(index).padStart(2, "0")}`}
            tick={(index) => index % 6 === 0}
            title={(index, value) =>
              `${String(index).padStart(2, "0")}:00–${String(index).padStart(2, "0")}:59 UTC · ${value.toLocaleString()} tokens`
            }
          />
        </Section>
        <Section title="By weekday">
          <Histogram
            values={stats.byWeekday}
            label={(index) => WEEKDAYS[index]}
            tick={() => true}
            title={(index, value) => `${WEEKDAYS[index]} · ${value.toLocaleString()} tokens`}
          />
        </Section>
      </div>

      <Section
        title="Token mix"
        note="Cache reads are most of the volume and a tenth of the price. A total that does not separate them points at the wrong culprit."
      >
        <Mix stats={stats} />
      </Section>

      <div className="pair">
        <Section title="By provider">
          <Bars rows={stats.byProvider} />
        </Section>
        <Section title="By harness">
          <Bars rows={stats.byHarness} />
        </Section>
      </div>

      <Section title="Top workspaces">
        <Bars rows={stats.byWorkspace} />
      </Section>
    </>
  );
}

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

function Section({
  title,
  note,
  children,
}: {
  title: string;
  note?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="panel">
      <h4>{title}</h4>
      {note && <p className="note">{note}</p>}
      {children}
    </section>
  );
}

/// Cost per day as columns.
///
/// Linear, unlike the calendar, and deliberately: this is a short window where the range
/// is narrow enough to read directly, and a log axis on money invites someone to compare
/// two bars and get the ratio wrong.
function Spend({ days }: { days: DayPoint[] }) {
  const peak = Math.max(...days.map((day) => day.costUsd ?? 0), 0.01);
  return (
    <div className="spend">
      {days.map((day) => (
        <i
          key={day.date}
          style={{ height: `${Math.max(2, ((day.costUsd ?? 0) / peak) * 100)}%` }}
          className={day.costUsd === null ? "unpriced" : undefined}
          title={`${day.date} · ${day.costUsd !== null ? `$${day.costUsd.toFixed(2)}` : "unpriced"} · ${day.tokens.toLocaleString()} tokens`}
        />
      ))}
    </div>
  );
}

function Histogram({
  values,
  label,
  tick,
  title,
}: {
  values: number[];
  label: (index: number) => string;
  /// Which columns get a written label. Twenty-four of them will not fit.
  tick: (index: number) => boolean;
  title: (index: number, value: number) => string;
}) {
  const peak = Math.max(...values, 1);
  return (
    <div className="histogram">
      <div className="histogram-bars">
        {values.map((value, index) => (
          <i
            key={index}
            style={{ height: `${Math.max(1, (value / peak) * 100)}%` }}
            title={title(index, value)}
          />
        ))}
      </div>
      <div className="histogram-axis" style={{ gridTemplateColumns: `repeat(${values.length}, 1fr)` }}>
        {values.map((_, index) => (
          <span key={index}>{tick(index) ? label(index) : ""}</span>
        ))}
      </div>
    </div>
  );
}

/// One stacked bar: what the tokens were.
function Mix({ stats }: { stats: StatsData }) {
  const { input, output, cacheRead, cacheWrite, reasoning } = stats.mix;
  const total = input + output + cacheRead + cacheWrite || 1;
  const parts = [
    { key: "cache read", value: cacheRead },
    { key: "cache write", value: cacheWrite },
    { key: "input", value: input },
    { key: "output", value: output },
  ];
  return (
    <>
      <div className="stack">
        {parts.map((part, index) => (
          <i
            key={part.key}
            className={`part-${index}`}
            style={{ width: `${(part.value / total) * 100}%` }}
            title={`${part.key} · ${part.value.toLocaleString()} tokens · ${((part.value / total) * 100).toFixed(1)}%`}
          />
        ))}
      </div>
      <dl className="legend">
        {parts.map((part, index) => (
          <div key={part.key}>
            <dt>
              <i className={`part-${index}`} />
              {part.key}
            </dt>
            <dd>{((part.value / total) * 100).toFixed(1)}%</dd>
          </div>
        ))}
        {reasoning > 0 && (
          <div title="Billed as output, not separately. Shown because it is the part of a bill nobody remembers asking for.">
            <dt>of which reasoning</dt>
            <dd>{((reasoning / Math.max(1, output)) * 100).toFixed(1)}%</dd>
          </div>
        )}
      </dl>
    </>
  );
}

/// Rows as bars, sorted by spend, each labelled with its own figure.
function Bars({ rows }: { rows: CostRow[] }) {
  if (rows.length === 0) return <p className="muted">Nothing recorded.</p>;
  const peak = Math.max(...rows.map((row) => row.costUsd ?? 0), 0.01);
  return (
    <div className="bars">
      {rows.map((row) => (
        <div className="bar" key={row.key}>
          <span className="bar-key" title={row.key}>
            {row.key}
          </span>
          <span className="bar-rail">
            <i style={{ width: `${((row.costUsd ?? 0) / peak) * 100}%` }} />
          </span>
          <span className="bar-value">
            {row.costUsd !== null ? `$${row.costUsd.toFixed(2)}` : "unpriced"}
          </span>
        </div>
      ))}
    </div>
  );
}

function Figure({
  label,
  value,
  exact,
}: {
  label: string;
  value: string;
  /// The unrounded figure, shown on hover where the card had to shorten it.
  exact?: string;
}) {
  return (
    <div className="figure" title={exact ?? value}>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}
