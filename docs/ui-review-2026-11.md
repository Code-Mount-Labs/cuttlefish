# UI review: November 2026

Three independent reviewers assessed visual hierarchy, product flow, and keyboard/mobile interaction over three rounds. The review retained the selected `b75caztznM` shadcn preset and used the actual Vite application, not a separate mockup.

| Round | Findings | Changes |
| --- | --- | --- |
| 1 | Competing navigation surfaces, double composer border, dense transcript, unexplained activity count, schedules requiring workflows | Stable labeled sidebar, generated InputGroup composer, grouped activity, independent prompt-based Routines, overflow session action |
| 2 | Browser screenshots exposed a CSS layer-order regression; mobile navigation close and new-chat controls overlapped | Centralized stylesheet ordering with explicit layers; browser assertions for padding and centering; separated mobile controls and focus |
| 3 | Repaired screens were coherent and readable; remaining improvements were the prompt editor height and schedule preview | Taller prompt field, explicit next-run preview, exact schedule anchor preservation on unrelated edits |

Final visual/product reviewers scored the repaired UI 8–8.5/10, with no remaining visual release blockers. These are reviewer judgments, not a substitute for accessibility testing. The browser contract checks desktop/mobile layout, dialog dismissal and focus, menus/tooltips under the deployed content security policy, attachments, safe Markdown, navigation, durable retry, and both local and managed-relay URL prefixes. Screen-reader testing remains a separate validation task.

Password changes are available in local Settings and hosted Account. Successful changes revoke existing sessions; authentication and restart behavior have separate Rust and UI tests.

A live managed-relay check subsequently caught a drawer-close focus race: Radix restored focus to the menu button after page navigation. Navigation dismissal now preserves destination focus, while Escape and outside dismissal still restore the trigger. The alpha.5 patch includes this correction.
