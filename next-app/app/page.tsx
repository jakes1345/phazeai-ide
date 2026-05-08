export default function Page() {
  return (
    <div className="flex min-h-svh p-6">
      <div className="flex max-w-md min-w-0 flex-col gap-4 text-sm leading-loose">
        <div>
          <h1 className="font-medium">PhazeAI companion web app</h1>
          <p>This scaffold is ready for Next.js + Tailwind development.</p>
          <p>
            Add UI components under <code>components/</code> and route pages
            under <code>app/</code>.
          </p>
          <button
            type="button"
            className="mt-2 inline-flex rounded-md border px-3 py-1.5 text-sm"
          >
            Starter button
          </button>
        </div>
        <div className="font-mono text-xs text-muted-foreground">
          (Press <kbd>d</kbd> to toggle dark mode)
        </div>
      </div>
    </div>
  )
}
