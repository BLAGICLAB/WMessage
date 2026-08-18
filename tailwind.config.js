/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      fontFamily: {
        sans: [
          "-apple-system",
          '"SF Pro Text"',
          '"PingFang SC"',
          '"Segoe UI"',
          '"Microsoft YaHei"',
          "sans-serif",
        ],
      },
    },
  },
  plugins: [],
};
