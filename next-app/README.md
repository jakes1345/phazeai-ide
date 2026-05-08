# next-app

Companion Next.js app scaffold in this repository.

## Scripts

```bash
npm install
npm run dev
npm run build
npm run lint
npm run typecheck
```

## Notes

- Uses App Router (`app/`) with Tailwind v4.
- Theme toggling is wired through `components/theme-provider.tsx` (press `d`).
- Keep shared UI in `components/`, domain hooks in `hooks/`, and utilities in `lib/`.
