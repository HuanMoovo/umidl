/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{vue,js,ts,jsx,tsx}'],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        // 主题强调色由 CSS 变量驱动（html[data-accent] 切换），无需重新编译
        umi: {
          50: 'rgb(var(--umi-a50) / <alpha-value>)',
          100: 'rgb(var(--umi-a100) / <alpha-value>)',
          200: 'rgb(var(--umi-a200) / <alpha-value>)',
          300: 'rgb(var(--umi-a300) / <alpha-value>)',
          400: 'rgb(var(--umi-a400) / <alpha-value>)',
          500: 'rgb(var(--umi-a500) / <alpha-value>)',
          600: 'rgb(var(--umi-a600) / <alpha-value>)',
          700: 'rgb(var(--umi-a700) / <alpha-value>)',
          800: 'rgb(var(--umi-a800) / <alpha-value>)',
          900: 'rgb(var(--umi-a900) / <alpha-value>)',
          950: 'rgb(var(--umi-a950) / <alpha-value>)',
        },
        accent: {
          cyan: '#22d3ee',
          pink: '#f472b6',
          lime: '#a3e635',
        },
      },
      fontFamily: {
        sans: ['"Inter"', '"HarmonyOS Sans SC"', '"Microsoft YaHei"', 'system-ui', 'sans-serif'],
        mono: ['"JetBrains Mono"', '"Cascadia Code"', 'Consolas', 'monospace'],
      },
      boxShadow: {
        glow: '0 0 24px rgb(var(--umi-a500) / 0.45)',
        'glow-cyan': '0 0 24px rgba(34, 211, 238, 0.4)',
        glass: 'var(--umi-shadow)',
      },
      animation: {
        'float-slow': 'float 8s ease-in-out infinite',
        'pulse-glow': 'pulseGlow 3s ease-in-out infinite',
        shimmer: 'shimmer 2s linear infinite',
      },
      keyframes: {
        float: {
          '0%, 100%': { transform: 'translateY(0px)' },
          '50%': { transform: 'translateY(-14px)' },
        },
        pulseGlow: {
          '0%, 100%': { opacity: '0.55' },
          '50%': { opacity: '1' },
        },
        shimmer: {
          '0%': { backgroundPosition: '-1000px 0' },
          '100%': { backgroundPosition: '1000px 0' },
        },
      },
    },
  },
  plugins: [],
}
