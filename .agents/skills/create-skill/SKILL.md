---
name: create-skill
description: Guideline and template for creating new custom skills for Maki.
---

# Create Skill

Use this skill when you need to define a new task-specific playbook, set of instructions, or standard operating procedure (SOP) for Maki to follow when executing specific types of tasks.

## Skill Folder Structure
Skills can be global (available across all projects) or local (project-specific).

### Local (Project-specific)
Create a new directory in:
`.agents/skills/<skill-name>/`

### Global
Create a new directory in:
`~/.config/skills/<skill-name>/`

Inside the directory, create:
* `SKILL.md` (Required): The playbook containing YAML frontmatter and markdown body instructions.

## SKILL.md Template

```markdown
---
name: <unique-skill-name-lowercase-kebab-case>
description: <concise one-sentence description of the skill and when it should be used>
---

# Skill Name

## Use this skill when
- Condition A
- Condition B

## Instructions
1. Step-by-step guideline.
2. Code style or verification rules.
```

## Best Practices
1. **Concise Description**: Keep the YAML description simple and concise, as it is displayed in the available skills list and used by the agent to decide when to load it.
2. **Kebab Case**: Use lowercase kebab-case for the skill name.
3. **No Bullshit**: Keep guidelines clear, specific, and actionable. Avoid generic instructions.
