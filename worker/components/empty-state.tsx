import type { ReactNode } from "react";

type EmptyStateProps = {
  readonly title: string;
  readonly description?: string;
  readonly hint?: ReactNode;
};

export function EmptyState({ title, description, hint }: EmptyStateProps) {
  return (
    <div className="libra-card libra-card-pad text-center">
      <p className="text-sm font-medium">{title}</p>
      {description && (
        <p className="mt-1 text-sm libra-text-muted">{description}</p>
      )}
      {hint && (
        <div className="mt-3 text-xs libra-text-faint">{hint}</div>
      )}
    </div>
  );
}
