import type { Config } from "tailwindcss";
import plugin from "tailwindcss/plugin";

const config: Config = {
  content: [
    "./app/**/*.{js,ts,jsx,tsx,mdx}",
    "./components/**/*.{js,ts,jsx,tsx,mdx}",
    "./lib/**/*.{js,ts,jsx,tsx,mdx}",
  ],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        bg: "var(--bg)",
        surface: "var(--surface)",
        "surface-elevated": "var(--surface-elevated)",
        accent: "var(--accent)",
        "accent-hover": "var(--accent-hover)",
        "accent-subtle": "var(--accent-subtle)",
        success: "var(--success)",
        danger: "var(--danger)",
        warning: "var(--warning)",
        info: "var(--info)",
        border: "var(--border)",
        "text-primary": "var(--text)",
        "text-muted": "var(--text-muted)",
      },
      fontFamily: {
        sans: ["DM Sans", "sans-serif"],
        mono: ["JetBrains Mono", "monospace"],
      },
      borderRadius: {
        sm: "4px",
        md: "8px",
        lg: "12px",
      },
    },
  },
  plugins: [],
};
export default config;
