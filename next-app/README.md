# next-app

An empty Next.js 16 scaffold (App Router, React 19, Tailwind 4). It isn't part of PhazeAI: nothing in the Rust workspace, the Makefile or CI builds or uses it.

What's here is a single placeholder page (`app/page.tsx`) with a button that does nothing, a `next-themes` provider that toggles dark mode when you press `d`, and empty `hooks/` and `lib/` folders. The page calls itself a "companion web app", but no such app has been built.

If you want to run it anyway:

```bash
npm install
npm run dev        # also: build, start, lint, typecheck, format
```
