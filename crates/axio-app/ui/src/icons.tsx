// The icon set, drawn rather than typed.
//
// Every glyph here shares one grid and one stroke: a 16-unit box, 1.5 units of
// stroke, round caps and round joins. That is the whole system — an icon that
// needs a different weight to read is the wrong icon, not a licence to add a
// second weight.
//
// These were characters before: `─`, `▢` and `✕` from whatever font the system
// resolved. A glyph is metrically centred for text, not for a 28-unit button;
// it inherits the font stack's own idea of weight; and it changes shape between
// machines. None of that is true of a path.

type IconProps = {
  /** Rendered size in pixels. The stroke scales with it, deliberately. */
  size?: number;
  className?: string;
};

function Icon({
  size = 16,
  className,
  children,
}: IconProps & { children: React.ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      // The icon is never the label. Every caller here sits inside a control
      // that already has an accessible name, so announcing the drawing again
      // would read the same thing twice.
      aria-hidden="true"
      focusable="false"
      className={className}
    >
      {children}
    </svg>
  );
}

export const IconMinimize = (p: IconProps) => (
  <Icon {...p}>
    <path d="M3.5 8h9" />
  </Icon>
);

export const IconMaximize = (p: IconProps) => (
  <Icon {...p}>
    <rect x="3.5" y="3.5" width="9" height="9" rx="1.5" />
  </Icon>
);

export const IconRestore = (p: IconProps) => (
  <Icon {...p}>
    <rect x="3" y="6" width="7" height="7" rx="1.5" />
    <path d="M6 6V4.5A1.5 1.5 0 0 1 7.5 3H11a2 2 0 0 1 2 2v3.5A1.5 1.5 0 0 1 11.5 10H10" />
  </Icon>
);

export const IconClose = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 4l8 8M12 4l-8 8" />
  </Icon>
);

/** Stopping a terminal. A square reads as "halt" where a cross reads as "dismiss". */
export const IconStop = (p: IconProps) => (
  <Icon {...p}>
    <rect x="4.5" y="4.5" width="7" height="7" rx="1.5" />
  </Icon>
);

export const IconTerminal = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 5.5L6.5 8 4 10.5" />
    <path d="M8.5 11h3.5" />
  </Icon>
);

export const IconBranch = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="5" cy="4" r="1.6" />
    <circle cx="5" cy="12" r="1.6" />
    <circle cx="11" cy="7" r="1.6" />
    <path d="M5 5.6v4.8" />
    <path d="M11 8.6c0 1.6-1.4 2.4-3.2 2.6" />
  </Icon>
);

export const IconRepo = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 3.5h6.5A1.5 1.5 0 0 1 12 5v7.5H5.5A1.5 1.5 0 0 1 4 11z" />
    <path d="M4 11a1.5 1.5 0 0 1 1.5-1.5H12" />
  </Icon>
);

/** A fold's chevron; points right when folded, rotated by the stylesheet. */
export const IconChevron = (p: IconProps) => (
  <Icon {...p}>
    <path d="M6 3.5 10.5 8 6 12.5" />
  </Icon>
);

/** Listening: an ear-ish arc with a dot. Marks a card the master box reaches. */
export const IconListen = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4.5 7.5a3.5 3.5 0 0 1 7 0c0 2.5-2.5 3-2.5 5.5" />
    <path d="M8 13.5v.01" />
  </Icon>
);

/** Split panes: a box with a divider. */
export const IconSplit = (p: IconProps) => (
  <Icon {...p}>
    <rect x="2.5" y="3.5" width="11" height="9" rx="1.5" />
    <path d="M8 3.5v9" />
  </Icon>
);

/** A grid of cards. */
export const IconGrid = (p: IconProps) => (
  <Icon {...p}>
    <rect x="2.5" y="2.5" width="4.5" height="4.5" rx="1" />
    <rect x="9" y="2.5" width="4.5" height="4.5" rx="1" />
    <rect x="2.5" y="9" width="4.5" height="4.5" rx="1" />
    <rect x="9" y="9" width="4.5" height="4.5" rx="1" />
  </Icon>
);

export const IconDiff = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4.5 3v6M2.5 5h4" />
    <path d="M9.5 11h4" />
  </Icon>
);

export const IconStart = (p: IconProps) => (
  <Icon {...p}>
    <path d="M8 3.5v9M3.5 8h9" />
  </Icon>
);

export const IconPlus = (p: IconProps) => (
  <Icon {...p}>
    <path d="M8 3.5v9M3.5 8h9" />
  </Icon>
);

export const IconMessage = (p: IconProps) => (
  <Icon {...p}>
    <path d="M3 3.5h10a1 1 0 0 1 1 1V10a1 1 0 0 1-1 1H7.5L4.5 13.5V11H3a1 1 0 0 1-1-1V4.5a1 1 0 0 1 1-1z" />
  </Icon>
);

export const IconSend = (p: IconProps) => (
  <Icon {...p}>
    <path d="M2.5 8h10M8.5 3.5L13 8l-4.5 4.5" />
  </Icon>
);

export const IconHistory = (p: IconProps) => (
  <Icon {...p}>
    <path d="M2.5 8a5.5 5.5 0 1 0 1.6-3.9M2.5 2.5v2.6h2.6M8 5v3.2l2.2 1.3" />
  </Icon>
);

/** A corner with a diagonal: the card's size. */
export const IconResize = (p: IconProps) => (
  <Icon {...p}>
    <path d="M3 13L13 3M13 8v5H8M3 8V3h5" />
  </Icon>
);

/** Two arrows out of a corner: expand into its own tab. */
export const IconExpand = (p: IconProps) => (
  <Icon {...p}>
    <path d="M9.5 3h3.5v3.5M13 3l-4 4M6.5 13H3V9.5M3 13l4-4" />
  </Icon>
);

/** Three dots: the row's menu. */
export const IconMore = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="4" cy="8" r="1.1" fill="currentColor" stroke="none" />
    <circle cx="8" cy="8" r="1.1" fill="currentColor" stroke="none" />
    <circle cx="12" cy="8" r="1.1" fill="currentColor" stroke="none" />
  </Icon>
);

/** Settings: a slider pair rather than a cog, on this grid a cog is mud. */
export const IconSettings = (p: IconProps) => (
  <Icon {...p}>
    <path d="M2.5 5h11M2.5 11h11" />
    <circle cx="6" cy="5" r="1.75" fill="currentColor" stroke="none" />
    <circle cx="10" cy="11" r="1.75" fill="currentColor" stroke="none" />
  </Icon>
);

/** A window with its left column: the rail toggle. */
export const IconRail = (p: IconProps) => (
  <Icon {...p}>
    <rect x="2.5" y="3.5" width="11" height="9" rx="1.5" />
    <path d="M6.5 3.5v9" />
  </Icon>
);
