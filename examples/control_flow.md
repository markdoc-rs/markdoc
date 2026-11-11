---
title: Fleet readiness
---

# {% $markdoc.frontmatter.title %}

{% if $fleet %}
{% count($fleet) %} robots are configured.
{% else /%}
No robots are configured.
{% /if %}

{% for $fleet as="robot" %}

## {% $robot.name %}

{% switch $robot.status %}

{% case "ok" %}
{% callout type="success" %}
Ready. Payload {% $robot.payload %} kg.
{% /callout %}
{% /case %}

{% case "service" %}
{% callout type="warning" %}
Out for service. Payload {% $robot.payload %} kg when it returns.
{% /callout %}
{% /case %}

{% case %}
{% callout type="danger" %}
Unknown status: {% $robot.status %}.
{% /callout %}
{% /case %}

{% /switch %}

{% for $robot.modules as="module" index="n" %}
- Slot {% $n %}: {% upper($module) %}
{% /for %}

{% /for %}
