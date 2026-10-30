# Annotation appearance display

Annotations without stored normal appearances use `pagura-appearance`, which
produces bounded PDF appearance content and resources. The ordinary interpreter,
paint preparation and rasterizer draw that content. Stored appearances retain
priority. The generator supports geometric markup, plain and supported rich
FreeText, icons and source text/choice/button widget values. It does not activate
media, execute actions or apply redactions.

This extraction reuses the authored implementation from PR #256 at
`3bef2b6ff537a0c742bba4c40d7e370c9c1fe5a6`, including its independent fixture
generators. It includes appearance generation, source-widget projection, normal
appearance selection, placement, and the narrowly required shared reservation
and page-view geometry helpers. It excludes that draft's facade editing/save
integration, host/viewer changes, broad ancestry, and unqualified benchmark
claims. Generated button states and rich-field editing are not newly promised by
the existing persistence facade. Shared form-model readers and filling helpers are
also updated to feed the common appearance generator; the existing editing
roundtrip tests cover those helpers and their persistence boundaries.

Rectangle scalars are resolved through the document object graph. Thus an
indirect numeric coordinate is valid in Strict and Repair modes. An inert Popup
with no rectangle or stored appearance has no flat-page paint: Repair reports
`annotation.popup-missing-rect` and skips it, while Strict refuses the same input.
No coordinates are inferred. A supplied appearance, malformed nonempty rectangle,
non-Popup missing rectangle, or failed object resolution retains its explicit
error.

The independent fixtures specify rational shape areas, original font contour
integrals, literal placement and text layouts. They distinguish specified PDF
geometry from reader-chosen icon artwork and layout policy. Passing those
fixtures proves the recorded subset, not universal similarity to another viewer.
The source data, positive fixtures and oracle bytes from the original branch are
preserved; qualification against the refactoring baseline is recorded separately.

Nonlinear generated path strokes (including cloudy outlines and nonlinear PDF
2.0 Ink), and dashed rounded borders currently refuse with the stable
`annotation.appearance` tag. Straight ordered cubic controls keep their ordinary
line behavior. Fills and solid rounded borders remain supported. Fourteen
independent controls exposed the separate source-tangent/arclength dependency:
their original tolerance-one bounds are retained in explicit accuracy-bound
vectors or the qualification follow-up record. The ordinary-content isolated
miter control is retained as a deferred vector for that raster follow-up; this annotation change
does not alter general page-path admission.

Empty text and unsigned signature widgets use their existing MK geometry after
proving Parent/Fields/Kids membership. Text resources, XFA and text-fit policy
are not selected for that geometric path. A separate unnamed widget cannot
shadow the owning field's nonempty value with an empty local V. Nonempty source
widgets keep the existing XFA boundary; complete field inspection still checks
its selected metadata, and form edits still enforce DoNotScroll fit.
