import React from "react";

/**
 * handy.stream wordmark: "handy" in the app's fg color, ".stream" in the
 * brand accent — visually distinct from upstream Handy's pink script logo.
 */
const HandyTextLogo = ({
  width,
  height,
  className,
}: {
  width?: number;
  height?: number;
  className?: string;
}) => {
  return (
    <svg
      width={width}
      height={height}
      className={className}
      viewBox="0 0 560 130"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
    >
      {/* waveform mark */}
      <g className="logo-primary">
        <rect x="8" y="46" width="14" height="38" rx="7" />
        <rect x="30" y="30" width="14" height="70" rx="7" />
        <rect x="52" y="14" width="14" height="102" rx="7" />
        <rect x="74" y="38" width="14" height="54" rx="7" />
        <rect x="96" y="52" width="14" height="26" rx="7" />
      </g>
      <text
        x="136"
        y="92"
        fontFamily="system-ui, -apple-system, sans-serif"
        fontSize="84"
        fontWeight="700"
        letterSpacing="-2"
        fill="currentColor"
      >
        handy
      </text>
      <text
        x="352"
        y="92"
        fontFamily="system-ui, -apple-system, sans-serif"
        fontSize="84"
        fontWeight="400"
        letterSpacing="-2"
        className="logo-primary"
      >
        .stream
      </text>
    </svg>
  );
};

export default HandyTextLogo;
