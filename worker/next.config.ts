import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  output: "standalone",
  reactStrictMode: true,
  typedRoutes: false,
  typescript: {
    ignoreBuildErrors: false,
  },
};

export default nextConfig;
