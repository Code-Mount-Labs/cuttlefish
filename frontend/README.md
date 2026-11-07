# Cuttlefish frontend

React + TypeScript, Vite, Tailwind CSS, and shadcn/ui. Both the machine dashboard and the hosted group dashboard use the same component and design system.

## Develop

This directory is the standalone Vite React TypeScript web project. Use Node 24 and npm, just as with a `create-vite` React TypeScript application.

```sh
cd frontend
npm ci
npm run dev
```

Open http://127.0.0.1:5173. For the hosted account and group dashboard, run `npm run dev:platform` instead. Both entry modes share the components and application features under `src/`.

The development server proxies `/api` to the local machine dashboard on port 64160. Set `CUTTLEFISH_DEV_API` in your shell to select a running API, for example `CUTTLEFISH_DEV_API=http://127.0.0.1:8080 npm run dev:platform`. The exact loopback development and preview origins are translated for the backend's same-origin checks. Credentials and provider keys belong in the backend configuration.

## Build and preview

```sh
npm run build
npm run preview
```

The normal production build outputs `dist/` with hashed assets. Preview opens on http://127.0.0.1:4173. For the hosted dashboard, use `npm run build:platform` followed by `npm run preview:platform`. Its output is `dist/platform/`. Deploy the selected output directory with `/api` routed to the matching backend.

The Rust distributions use a separate packaging command, `npm run build:embedded`. It generates `../web/local/{index.html,app.js,app.css}` and `../web/{index.html,app.js,app.css}` with stable filenames required by the Rust asset handlers. Those generated assets are checked in so Rust users can compile without Node. Containers and CI run this packaging command. Edit `src/`, then regenerate the embedded assets when shipping changes.

## Components and checks

`components.json` configures the official shadcn CLI for Vite, Tailwind v4, the Radix base, and the `@/` import alias. Components live in `src/components/ui/` and are composed by features. Forms use `FieldGroup`, `Field`, `FieldLabel`, `FieldDescription`, `Input`, `NativeSelect`, and `Checkbox`. Buttons, textareas, and dialogs use the same shadcn primitives and semantic theme tokens.

```sh
npx shadcn@latest info
npx shadcn@latest add <component>
npm run typecheck
npm run lint
npm run coverage
npm run test:vite # after building both standalone entry points
npm run format:check
```

Review registry updates before applying them. This project customizes dialog scroll containment for its Content Security Policy and retains only the field primitives used by the application. The coverage gate requires 100% statements, branches, functions, and lines, including the checked-in UI components.

TypeScript separates browser source (`tsconfig.app.json`), Node build tools (`tsconfig.node.json`), and test types (`tsconfig.test.json`). `tsconfig.json` references all three. The `@/` alias is configured for TypeScript, Vite, Vitest, and shadcn.

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

The shadcn project conventions were verified with the official [shadcn skill](https://github.com/shadcn-ui/ui/blob/main/skills/shadcn/SKILL.md).
