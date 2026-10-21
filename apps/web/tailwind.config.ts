import type { Config } from "tailwindcss";
import containerQueries from "@tailwindcss/container-queries";
import tailwindcssAnimate from "tailwindcss-animate";

const remText = (fontSizePx: number, lineHeightPx: number): [string, string] => [
  `${fontSizePx / 16}rem`,
  `${lineHeightPx / 16}rem`,
];

export default {
  darkMode: ["class"],
  content: ["./pages/**/*.{ts,tsx}", "./components/**/*.{ts,tsx}", "./app/**/*.{ts,tsx}", "./src/**/*.{ts,tsx}"],
  prefix: "",
  theme: {
    container: {
      center: true,
      padding: "2rem",
      screens: {
        "2xl": "1400px",
      },
    },
    extend: {
      // Primary text/borders use readable ink; backgrounds use the brand fill.
      backgroundColor: {
        primary: {
          DEFAULT: "hsl(var(--primary-fill))",
          foreground: "hsl(var(--primary-foreground))",
        },
      },
      fontFamily: {
        sans: ['ui-sans-serif', 'system-ui', '-apple-system', 'Segoe UI', 'Arial', 'sans-serif'],
        mono: ['ui-monospace', 'SFMono-Regular', 'Cascadia Code', 'Menlo', 'Consolas', 'monospace'],
      },
      fontSize: {
        'xs': remText(13, 20),
        'sm': remText(15, 22),
        'base': remText(16, 24),
        'lg': remText(18, 26),
        'xl': remText(24, 32),
        '2xl': remText(24, 32),
        '3xl': remText(28, 36),
      },
      colors: {
        border: "hsl(var(--border))",
        "border-muted": "hsl(var(--border-muted))",
        "border-default": "hsl(var(--border-default))",
        "border-subtle": "hsl(var(--border-subtle))",
        input: "hsl(var(--input))",
        ring: "hsl(var(--ring))",
        background: "hsl(var(--background))",
        foreground: "hsl(var(--foreground))",
        surface: {
          DEFAULT: "hsl(var(--surface))",
          raised: "hsl(var(--surface-raised))",
          0: "hsl(var(--surface-0))",
          1: "hsl(var(--surface-1))",
          2: "hsl(var(--surface-2))",
          3: "hsl(var(--surface-3))",
        },
        text: {
          primary: "hsl(var(--text-primary))",
          secondary: "hsl(var(--text-secondary))",
          muted: "hsl(var(--text-muted))",
          subtle: "hsl(var(--text-subtle))",
          inverse: "hsl(var(--text-inverse))",
        },
        primary: {
          DEFAULT: "hsl(var(--primary))",
          foreground: "hsl(var(--primary-foreground))",
        },
        secondary: {
          DEFAULT: "hsl(var(--secondary))",
          foreground: "hsl(var(--secondary-foreground))",
        },
        destructive: {
          DEFAULT: "hsl(var(--destructive))",
          foreground: "hsl(var(--destructive-foreground))",
        },
        muted: {
          DEFAULT: "hsl(var(--muted))",
          foreground: "hsl(var(--muted-foreground))",
        },
        accent: {
          DEFAULT: "hsl(var(--accent))",
          foreground: "hsl(var(--accent-foreground))",
          hover: "hsl(var(--accent-hover))",
          active: "hsl(var(--accent-active))",
        },
        popover: {
          DEFAULT: "hsl(var(--popover))",
          foreground: "hsl(var(--popover-foreground))",
        },
        card: {
          DEFAULT: "hsl(var(--card))",
          foreground: "hsl(var(--card-foreground))",
        },
        status: {
          info: "hsl(var(--status-info))",
          positive: "hsl(var(--status-positive))",
          warning: "hsl(var(--status-warning))",
          negative: "hsl(var(--status-negative))",
          video: "hsl(var(--status-video))",
        },
        sidebar: {
          DEFAULT: "hsl(var(--sidebar-background))",
          foreground: "hsl(var(--sidebar-foreground))",
          primary: "hsl(var(--sidebar-primary))",
          "primary-foreground": "hsl(var(--sidebar-primary-foreground))",
          accent: "hsl(var(--sidebar-accent))",
          "accent-foreground": "hsl(var(--sidebar-accent-foreground))",
          border: "hsl(var(--sidebar-border))",
          ring: "hsl(var(--sidebar-ring))",
        },
      },
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
      },
      spacing: {
        // NOTE: do not add a bare `xs` key here — under Tailwind v4 it leaks
        // into `max-w-xs` (shadowing the default 20rem with 2px), which silently
        // collapses every `max-w-xs` container app-wide. Spacing steps use the
        // `-space` suffix to stay out of the width/max-width namespaces.
        'sm-space': '4px',
        'md-space': '8px',
        'lg-space': '12px',
        'xl-space': '16px',
        '2xl-space': '24px',
        '3xl-space': '32px',
      },
      boxShadow: {
        'base': 'var(--shadow-sm)',
        'md': 'var(--shadow-md)',
        'lg': 'var(--shadow-lg)',
        'inset': 'inset 0 1px 0 0 hsl(0 0% 0% / 0.2)',
        'inner-light': 'inset 0 1px 0 0 hsl(0 0% 0% / 0.2)',
      },
      keyframes: {
        "accordion-down": {
          from: { height: "0" },
          to: { height: "var(--radix-accordion-content-height)" },
        },
        "accordion-up": {
          from: { height: "var(--radix-accordion-content-height)" },
          to: { height: "0" },
        },
        "skeleton-shimmer": {
          "0%": { transform: "translateX(-100%)" },
          "100%": { transform: "translateX(100%)" },
        },
        "fade-in": {
          "0%": { opacity: "0", transform: "translateY(6px)" },
          "100%": { opacity: "1", transform: "translateY(0)" },
        },
        "slide-up": {
          "0%": { opacity: "0", transform: "translateY(12px)" },
          "100%": { opacity: "1", transform: "translateY(0)" },
        },
        "scale-in": {
          "0%": { opacity: "0", transform: "scale(0.96)" },
          "100%": { opacity: "1", transform: "scale(1)" },
        },
        "pulse-soft": {
          "0%, 100%": { opacity: "1" },
          "50%": { opacity: "0.7" },
        },
        "slide-in-left": {
          "0%": { opacity: "0", transform: "translateX(-100%)" },
          "100%": { opacity: "1", transform: "translateX(0)" },
        },
      },
      animation: {
        "accordion-down": "accordion-down 0.2s ease-out",
        "accordion-up": "accordion-up 0.2s ease-out",
        "fade-in": "fade-in 0.25s ease-out",
        "slide-up": "slide-up 0.3s ease-out",
        "scale-in": "scale-in 0.2s ease-out",
        "pulse-soft": "pulse-soft 2s ease-in-out infinite",
        "slide-in-left": "slide-in-left 0.25s ease-out",
      },
    },
  },
  plugins: [tailwindcssAnimate, containerQueries],
} satisfies Config;
