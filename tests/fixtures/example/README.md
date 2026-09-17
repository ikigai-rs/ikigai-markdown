Invented documents for `mappings/example/`, in the decision-record format that mapping
describes. They exist to be tested against, and each plants something a test looks for:

- `0001` writes its record fields as YAML frontmatter, `0002` as a `## Record` bullet
  list, and `0003` as frontmatter again, so both shapes are exercised.
- `0003`'s `related:` names `0009`, which does not exist. Only the lift profile's split
  makes that token visible to a query.
- `0002`'s prose cites `DR-0007`, which does not exist either.
