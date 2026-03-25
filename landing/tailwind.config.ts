import type { Config } from "tailwindcss";

const config: Config = {
  content: [
    "./app/**/*.{js,ts,jsx,tsx,mdx}",
    "./components/**/*.{js,ts,jsx,tsx,mdx}",
  ],
  theme: {
    extend: {
      colors: {
        primary: "#4a9eff",
        accent: "#f59e0b",
        rust: "#ff6b35",
        vault: "#7c3aed",
        hot: "#22c55e",
      },
    },
  },
  plugins: [],
};
export default config;
