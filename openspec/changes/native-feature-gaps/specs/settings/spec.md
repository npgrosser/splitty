## ADDED Requirements

### Requirement: Native Tab Bar Settings
The settings SHALL hold a group `nativeTabs` for the tab bars of the native app, apart from `tabs`, which stays the web client's: `showSplit`, `showFloat` and `showMaximize`, true by default; `closeButton`, `always` by default or `active` for the selected tab of a group only; and `locked`, false by default, which keeps tabs from being dragged. The native window SHALL follow them and offer them under its settings.

#### Scenario: The float button turned off
- **WHEN** the user turns `nativeTabs.showFloat` off in the native settings
- **THEN** the native tab bars show no float button, and the web client's tab bars are as before
