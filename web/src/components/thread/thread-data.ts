export interface BranchTopologyItem {
  name: string;
  tone: "muted" | "active" | "soft";
  hasConnector: boolean;
  hasSplit: boolean;
  headLabel?: string;
}

export interface Collaborator {
  code: string;
  specialty: string;
  idle?: boolean;
}

export interface FeaturedChange {
  id: string;
  updatedAt: string;
  status: string;
  filePath: string;
  title: string;
  description: string;
  author: {
    name: string;
    initials: string;
  };
  impact: Array<{
    label: string;
    value: string;
    tone?: "default" | "positive";
  }>;
  comments: string;
}

export interface CompactChange {
  id: string;
  tone: "review" | "merged" | "archived";
  updatedAt: string;
  status: string;
  title: string;
  description: string;
  meta: Array<{
    label: string;
    value: string;
  }>;
}

export const repositorySource = {
  name: "MONOREPO_CORE_V4",
  id: "994-2A-X",
};

export const currentBranch = "feat/tensor-optimization";

export const commandPreview =
  "list changes --limit=50 --status=all --sort=desc";

export const branchTopology: BranchTopologyItem[] = [
  {
    name: "main",
    tone: "muted",
    hasConnector: false,
    hasSplit: false,
  },
  {
    name: "feat/tensor-opt",
    tone: "active",
    hasConnector: true,
    hasSplit: true,
    headLabel: "HEAD",
  },
  {
    name: "fix/auth-headers",
    tone: "soft",
    hasConnector: true,
    hasSplit: false,
  },
  {
    name: "chore/deps-bump",
    tone: "soft",
    hasConnector: true,
    hasSplit: false,
  },
];

export const collaborators: Collaborator[] = [
  { code: "AGNT-01", specialty: "Refactor" },
  { code: "AGNT-04", specialty: "Security" },
  { code: "AGNT-09", specialty: "Docs" },
  { code: "AGNT-12", specialty: "Idle", idle: true },
];

export const featuredChange: FeaturedChange = {
  id: "CL-4092",
  updatedAt: "JUST NOW",
  status: "IN PROGRESS",
  filePath: "optimization_routine_v2.py",
  title: "Vectorized implementation for GPU offload",
  description:
    "Analyzing computational complexity. Detected O(n^2) loop in vector transformation. Restructuring for parallel execution via CUDA cores. Memory alignment adjusted for 64-byte boundaries.",
  author: {
    name: "AGNT-04",
    initials: "A4",
  },
  impact: [
    { label: "Files Changed", value: "3" },
    { label: "Coverage Delta", value: "+2.4%", tone: "positive" },
    { label: "Latency", value: "-45ms", tone: "positive" },
  ],
  comments: "2 comments",
};

export const compactChanges: CompactChange[] = [
  {
    id: "CL-4091",
    tone: "review",
    updatedAt: "14 MIN AGO",
    status: "IN REVIEW",
    title: "Sanitization of input headers",
    description:
      "Security vulnerability detected in header parsing logic. Agent implemented strict type checking and removed redundant middleware layers that exposed raw stack traces in production environments.",
    meta: [
      { label: "Author", value: "AGNT-01" },
      { label: "Coverage", value: "98.4%" },
    ],
  },
  {
    id: "CL-4090",
    tone: "merged",
    updatedAt: "1 HR AGO",
    status: "MERGED",
    title: "Initial scaffold setup for Python 3.11",
    description:
      "Project initialization. Configured base environments and established communication protocols for AI agent integration. Added pyproject.toml and poetry.lock.",
    meta: [
      { label: "Author", value: "USER" },
      { label: "Files", value: "12" },
    ],
  },
  {
    id: "CL-4089",
    tone: "archived",
    updatedAt: "3 HR AGO",
    status: "ARCHIVED",
    title: "Legacy adapter removal",
    description:
      "Deprecated v1 adapters. This change has been superseded by CL-4091.",
    meta: [{ label: "Author", value: "AGNT-02" }],
  },
];
