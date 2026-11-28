# Design

## The rankings are tables of the query's own rows

A table panel receives a Loki instant metric query already as one table: a `Time` column, one column per label (`name` or `client`) and a `Value #A` column, one row per series. The panels therefore only hide `Time`, rename the label column to **Name** or **Client** and `Value #A` to **Queries**, and sort by **Queries**, descending. A reduce step (series to rows) must not be used: on a table it reduces each column to a single value and leaves two rows. The list table in the overview already relies on the same table shape.

## The selector override is a text replacement of a known default

The packaged default `{app_kubernetes_io_name="hickory-filter"}` appears only inside the Log stream selector variable (its query, current value and option). The chart replaces that JSON-escaped text with the value, itself JSON-escaped, so the rest of the file is passed through byte for byte and the golden renders of the default case stay stable. Parsing and re-serialising the JSON in the template would reorder keys and rewrite the whole file. If the value is set and the default isn't found, the render fails with a message naming `dashboard.logSelector`.

## One value for both resources

The operator resource and the sidecar ConfigMap carry the same file, so the value lives in a shared `dashboard` block rather than in each resource's block.
