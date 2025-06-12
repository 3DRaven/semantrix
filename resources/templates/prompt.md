{# Information about Discovered Project Symbols #}

## Semantic Rules

{% if semantic_rules is defined and semantic_rules | length > 0 %}
{% for rule in semantic_rules %}
- {{ rule }}
{% endfor %}
{% else %}
_No additional semantic rules specified._
{% endif %}

## Semantic Symbols

{% if semantic_symbols | length == 0 %}
**No semantic symbols found.**
{% else %}
{% for symbol in semantic_symbols %}
---

**Name:** `{{ symbol.name }}`
- **Kind:** `{{ symbol.kind }}`
- **Container:** {% if symbol.container_name is defined and symbol.container_name | default(value="") != "" %}{{ symbol.container_name }}{% else %}(none){% endif %}
- **Location:** 
    - URI: `{{ symbol.location.uri }}`
    - Range: lines {{ symbol.location.range.start.line + 1 }}-{{ symbol.location.range.end.line + 1 }}, columns {{ symbol.location.range.start.character + 1 }}-{{ symbol.location.range.end.character + 1 }}
{% if symbol.code is defined and symbol.code | default(value="") != "" %}
- **Code:**
```
{{ symbol.code }}
```
{% endif %}

{% endfor %}
{% endif %}

---

## Fuzzy Rules

{% if fuzzy_rules is defined and fuzzy_rules | length > 0 %}
{% for rule in fuzzy_rules %}
- {{ rule }}
{% endfor %}
{% else %}
_No additional fuzzy rules specified._
{% endif %}

## Fuzzy Symbols

{% if fuzzy_symbols | length == 0 %}
**No fuzzy symbols found.**
{% else %}
{% for symbol in fuzzy_symbols %}
---

**Name:** `{{ symbol.name }}`
- **Kind:** `{{ symbol.kind }}`
- **Container:** {% if symbol.container_name is defined and symbol.container_name | default(value="") != "" %}{{ symbol.container_name }}{% else %}(none){% endif %}
- **Location:** 
    - URI: `{{ symbol.location.uri }}`
    - Range: lines {{ symbol.location.range.start.line + 1 }}-{{ symbol.location.range.end.line + 1 }}, columns {{ symbol.location.range.start.character + 1 }}-{{ symbol.location.range.end.character + 1 }}
{% if symbol.code is defined and symbol.code | default(value="") != "" %}
- **Code:**
```
{{ symbol.code }}
```
{% endif %}

{% endfor %}
{% endif %}

---

### Guidance for Code Generation

- When generating code based on the discovered symbols, **always respect the rules listed under "Semantic Rules" and "Fuzzy Rules"** for each respective symbol category.
- The rules are provided as `Vec` collections and must be strictly followed during code synthesis, refactoring, or analysis.
- **Reuse already implemented entities** from the lists above whenever possible, instead of generating new ones.
- If a rule set is empty, proceed with standard code generation practices for that symbol category.

Use this symbol and rule list to analyze the project structure, search for relevant entities, and guide meaningful, context-aware code-related responses.