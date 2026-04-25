type Props = {
  size?: number;
  className?: string;
};

export function BrandMark({ size = 18, className }: Props) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      className={className}
    >
      <path
        d="M5 4h3v13h8v3H5z"
        stroke="currentColor"
        strokeWidth={1.8}
        strokeLinejoin="round"
      />
      <circle cx="17" cy="6" r="2" fill="currentColor" />
    </svg>
  );
}
