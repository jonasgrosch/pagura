"""Author-side mutation ledger and ISO 32000-2 permission oracle.

This consumes author declarations and successive object bodies, never a PDF
reader or engine result. Table 257 permits specified operations; Table 259 and
12.8.2.4 restrict every modification of selected fields, including inherited
configuration. Unknown operations remain unresolved. Output spellings are only
the serialization format used by the Rust assertions.
"""
from dataclasses import dataclass
import re


@dataclass(frozen=True)
class Edit:
    target: str
    operation: str
    owners: tuple = ()


def edit(target, operation, owners=()):
    return Edit(str(target), operation, owners)


def field_names(objects):
    """Names come from the generator's simple, unescaped authored dictionaries."""
    partial, parents = {}, {}
    for target, body in objects.items():
        if not isinstance(body, bytes):
            continue
        name = re.search(rb'/T \(([^()]*)\)', body)
        if name:
            partial[target] = name[1].decode()
        parent = re.search(rb'/Parent (\d+) 0 R', body)
        if parent:
            parents[target] = parent[1].decode()
    def qualified(target):
        parent = parents.get(target)
        return (qualified(parent) + '.' if parent in partial else '') + partial[target]
    return {target: qualified(target) for target in partial}


def is_locked(pdf, name):
    action, names = pdf.author_selection
    if action is None:
        return False
    included = any(name == n or name.startswith(n + '.') for n in names)
    return action == 'All' or included != (action == 'Exclude')


def owners(pdf, change, before, after):
    if change.owners:
        return change.owners
    mappings = [field_names(state) for state in (pdf.author_states[0], before, after)]
    names = {n for mapping in mappings for n in mapping.values()}
    target = change.target
    roots = {mapping[target] for mapping in mappings if target in mapping}
    if roots:
        return tuple(n for n in names if any(n == own or n.startswith(own + '.') for own in roots))
    if target == '4' and change.operation == 'configuration':
        return tuple(names)
    if change.operation in ('fill', 'appearance', 'widget-appearance'):
        return ('Amount',)
    # The authored annotation containers in these cases remove Amount's widget.
    if change.operation == 'annotation' and target in ('3', '12'):
        a, b = before.get(target, b''), after.get(target, b'')
        if b'7 0 R' in a and b'7 0 R' not in b:
            return ('Amount',)
    return ()


def decision(pdf, change, before, after, changed_values):
    operation = change.operation
    owner_names = owners(pdf, change, before, after)
    rule = {'fill': 'FormFill', 'appearance': 'FormFill', 'sign': 'Signing', 'signing-appearance': 'Signing',
            'annotation': 'Annotation', 'annotation-appearance': 'Annotation',
            'widget-appearance': 'Annotation', 'configuration': 'DocumentStructure',
            'page-content': 'DocumentStructure', 'identifier': 'CrossReference',
            'bookkeeping': 'CrossReference'}.get(operation, 'Unclassified')
    permitted = operation in ('fill', 'sign', 'signing-appearance', 'annotation', 'annotation-appearance',
                              'widget-appearance', 'identifier', 'bookkeeping')
    unresolved = operation in ('unresolved', 'template', 'information')
    if operation == 'appearance':
        permitted = bool(owner_names) and all(n in changed_values for n in owner_names)
        unresolved = not permitted
        if unresolved and pdf.author_permission == 3:
            permitted, unresolved, rule = True, False, 'Annotation'
        elif unresolved:
            rule = 'Unclassified'
    if operation == 'sign' and change.target == '4':
        flags = lambda state: int(re.search(rb'/SigFlags (\d+)', state['4'])[1])
        prior, next_value = flags(before), flags(after)
        if prior & ~next_value or (prior ^ next_value) & ~3:
            permitted, rule = False, 'DocumentStructure'
    if operation == 'identifier':
        # 14.4: the permanent identifier stays fixed; only the second changes.
        first = lambda state: re.search(rb'/ID \[([^ ]+)', state.get('trailer', b''))[1]
        permitted = first(before) == first(after)
        if not permitted:
            rule = 'DocumentStructure'
    if operation != 'bookkeeping' and rule != 'CrossReference' and not unresolved:
        if pdf.author_permission == 1:
            permitted, rule = False, 'NoChanges'
        elif rule == 'Annotation' and pdf.author_permission == 2:
            permitted = False
    doc = 'Inconclusive' if unresolved else 'Allowed' if permitted else 'Disallowed'
    if pdf.author_unknown_transform and doc != 'Disallowed':
        doc = 'Inconclusive'
    if any(is_locked(pdf, name) for name in owner_names):
        field = 'Disallowed'
    elif pdf.author_field_unknown:
        field = 'Inconclusive'
    elif pdf.author_selection[0] is None:
        field = 'NotApplicable'
    elif operation == 'unresolved':
        field = 'Inconclusive'
    else:
        field = 'Allowed'
    body = after.get(change.target, b'')
    prior = before.get(change.target, b'')
    def appearance_present(body, state):
        entry = re.search(rb'/AP\s+(null\b|\d+ \d+ R)?', body)
        if entry is None:
            return False
        value, seen = entry[1], set()
        while value and (reference := re.fullmatch(rb'(\d+) (\d+) R', value)):
            target = reference[1].decode() + (':' + reference[2].decode() if int(reference[2]) else '')
            assert target not in seen, 'cyclic authored appearance reference'
            seen.add(target)
            value = state.get(target, b'null').strip()
        return value != b'null'
    overlay = (operation == 'annotation-appearance'
               or (operation == 'annotation' and appearance_present(body, after))
               or (rule in ('Signing', 'Annotation')
                   and (b'/FT ' in body or b'/Subtype /Widget' in body
                        or operation in ('widget-appearance', 'signing-appearance')))
               or (not appearance_present(prior, before) and appearance_present(body, after)))
    return doc, field, rule, 'overlay' if overlay else 'clear'


def aggregate(decisions):
    values = {value for pair in decisions for value in pair[:2]}
    return next((v for v in ('Disallowed', 'Inconclusive', 'Allowed') if v in values), 'NoChanges')


def expectations(pdf, declarations):
    if not pdf.author_history_intact:
        return b'Inconclusive\n', b'Inconclusive\n'
    updates = len(pdf.author_states) - 1
    steps = [[] for _ in range(updates)]
    current = 0
    for declaration in declarations:
        if isinstance(declaration, str):
            current = updates - 1 - int(declaration[1:])
        else:
            steps[current].append(declaration)
    seen, cumulative, outputs = {}, 'NoChanges', [[], []]
    for index, declarations in enumerate(steps, 1):
        before, after = pdf.author_states[index-1:index+1]
        changed = {target for target in before.keys() | after.keys() if before.get(target) != after.get(target)}
        declared = {change.target for change in declarations}
        assert changed == declared, (changed, declared)
        for declaration in declarations:
            seen[declaration.target] = declaration
        comparisons = []
        for start, intents in ((before, declarations), (pdf.author_states[0], seen.values())):
            changed_values = set()
            for change in intents:
                if change.operation == 'fill' and start.get(change.target) != after.get(change.target):
                    changed_values.update(owners(pdf, change, start, after))
            rows, decisions = [], []
            for change in sorted(intents, key=lambda c: (c.target == 'trailer', tuple(map(int, c.target.split(':'))) if c.target != 'trailer' else ())):
                target = change.target
                a, b = start.get(target), after.get(target)
                if a == b:
                    continue
                kind = 'Added' if a is None else 'Removed' if b is None else 'Modified'
                result = decision(pdf, change, start, after, changed_values)
                decisions.append(result)
                rows.append('|'.join((target, kind, *result)))
            comparisons.append((rows, decisions))
        cumulative = aggregate([(cumulative, 'NotApplicable')] + [d for _, ds in comparisons for d in ds])
        for output, (rows, _) in zip(outputs, comparisons):
            if updates > 1:
                output.append(f'@{updates-index}|{cumulative}')
            output.extend(rows)
    return tuple(('\n'.join([cumulative] + rows) + '\n').encode() for rows in outputs)
