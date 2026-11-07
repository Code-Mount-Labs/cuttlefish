# Cuttlefish frontend

React + TypeScript, Vite, Tailwind CSS, and shadcn/ui. Both the machine dashboard and the hosted group dashboard use the same component and design system.

## Develop

Use Node 24 and npm. From the repository root:

```sh
npm ci --prefix frontend
npm run dev --prefix frontend
```

The development server proxies `/api` to the local machine dashboard on port 64160. `CUTTLEFISH_DEV_API=http://127.0.0.1:8080 npm run dev --prefix frontend -- --mode platform` selects the hosted group dashboard. The loopback-only development proxy translates only its own exact browser origin, keeping the backend origin checks intact.

```sh
npm run format:check --prefix frontend
npm run lint --prefix frontend
npm run test --prefix frontend
npm run coverage --prefix frontend
npm run build --prefix frontend
```

The build produces `web/local/{index.html,app.js,app.css}` and `web/{index.html,app.js,app.css}`. These generated assets are checked in because the Rust binaries embed them and must build without a JavaScript toolchain. Edit `frontend/src`, never the generated files. CI rebuilds and rejects stale assets. Containers rebuild from the locked frontend source. Browser contract tests exercise the same embedded assets, including the installation-specific relay prefix.

## Boundaries

- `app/`: authenticated application roots; logout cancels and clears cached data.
- `features/chat/`: transcript, composer, attachments, durable request recovery, activity.
- `features/settings/`: local configuration; credentials are sent only to the machine API.
- `features/platform/`: groups, installations, single-use enrollment invitations.
- `components/ui/`: shadcn/ui components, owned by this repository; only visual primitives.
- `components/`: reusable application components, error boundary, semantic form fields.
- `lib/contracts.ts`: Zod response contracts and inferred TypeScript types. Untrusted JSON is validated at the network boundary.
- `lib/files.ts`: bounded chunk transfer, integrity checks, download lifecycle.
- `hooks/`: browser resource subscriptions with explicit cleanup.

TanStack Query owns server data, cancellation and polling. Component state owns drafts and local interaction. Mutation retries are disabled: dispatch retries reuse the exact durable command ID and uploaded file references. Providers receive no extra calls from UI polling. The production CSP remains unchanged: Zod uses its interpreter and the dialog overlay uses CSS scroll containment without injected style tags. No agent content becomes raw HTML; external images are not fetched from generated Markdown.

## Design

The primary action is writing a message. Keep the sidebar compact, the transcript readable, and progress near the task it describes. No promotional subtitles, decorative metrics, gradients, or invented progress. The only automatic motion is an active task indicator, disabled with reduced motion. Forms have visible labels, icon buttons have accessible names, mobile navigation and dialogs manage focus through Radix, and failure feedback includes an action the user can take.

Design guidance reviewed: https://github.com/anthropics/skills/blob/main/skills/frontend-design/SKILL.md . shadcn/ui components originate from https://ui.shadcn.com (MIT).
