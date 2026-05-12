import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  output: "standalone",
  reactStrictMode: true,
  experimental: {
    typedRoutes: false,
  },
  typescript: {
    ignoreBuildErrors: false,
  },
};

export default nextConfig;
