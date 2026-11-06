# ADR-0032: The client draws these SwiftUI controls under their SwiftUI names

`BuiltinComponentType` in `a2ui-swift` holds 18 components (Text, Image, Icon, Video, AudioPlayer, Row, Column, List, Card, Tabs, Divider, Modal, Button, CheckBox, TextField, DateTimeInput, ChoicePicker, Slider) and none of the controls below. Each is a SwiftUI view or modifier, so the client draws SwiftUI's own, and a page names it as SwiftUI does (ADR-0031).

ProgressView, Gauge, Menu, Link, Form, Section, LabeledContent, Grid, GridRow, Stepper, ColorPicker, Popover, Alert, ConfirmationDialog, NavigationSplitView, NavigationStack, HSplitView, VSplitView, Toolbar, Chart, Map, ShareLink, PasteButton.

A property is the SwiftUI initializer's argument of the same name, and `children` is its content closure. `Popover`, `Alert`, `ConfirmationDialog` and `Toolbar` are modifiers in SwiftUI; the component holds the content they apply to. A control that holds a value draws the value the page sent and raises its `action` with the new one as `value`.
