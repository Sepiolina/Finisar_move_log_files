# `ui/frontend/index.html`

[Reference index](../../README.md) · [Source](../../../../ui/frontend/index.html)

Static DOM contract for `app.js`, loaded with defer after `styles.css`. Defines Archive, Extract, Viewer, and Settings tabs; flow containers; result panels; file/DB/email inputs; activity log; preview; and toast host. `data-tab` selects navigation, `data-pick` routes native dialogs, `data-kind` selects file filters, and `data-do` routes actions. IDs are queried directly by JavaScript, so renaming them requires coordinated changes. Default subfolders are BURNIN, ESS, JTAG, STM, and Xray. Contains no functions.
