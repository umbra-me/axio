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
