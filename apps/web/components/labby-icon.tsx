export function LabbyIcon({ size = 32, className }: { size?: number; className?: string }) {
  const height = size
  const width = size * (48 / 51)

  return (
    <svg
      width={width}
      height={height}
      viewBox="0 0 48 51"
      fill="none"
      role="img"
      aria-label="Labby"
      className={className}
    >
      <title>Labby</title>
      <path d="M8 13L24 7L40 13L24 19Z" fill="var(--aurora-border-strong)" opacity="0.96" />
      <path d="M8 21L24 15L40 21L24 27Z" fill="var(--aurora-accent-deep)" opacity="0.92" />
      <path d="M8 29L24 23L40 29L24 35Z" fill="var(--aurora-accent-primary)" opacity="0.88" />
      <path d="M8 37L24 31L40 37L24 43Z" fill="var(--aurora-accent-strong)" opacity="0.9" />
    </svg>
  )
}
