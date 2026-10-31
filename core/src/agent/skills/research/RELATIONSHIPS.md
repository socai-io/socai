Research relationship definitions (platform-neutral):

Each edge has a dimension, kind, specific label, concise basis and source references.
Dimensions:
- identity: records describing the same entity across sources. Match identifiers,
  repository/domain links or explicit attribution, never a name alone. Preserve
  original records; an identity edge is not permission to merge uncertain entities.
- people: founder/team membership, shared affiliation, investment, collaboration,
  or a documented shared event. Co-appearance does not imply partnership.
- problem: the same concrete job, pain point or workflow; explain the common job.
- audience: overlapping target users or distribution/usage setting. Do not call
  products competitors solely because they share an industry label.
- technology: a specific shared or contrasting architecture, deployment method,
  memory approach, interaction method or technical constraint.
- ecosystem: observed dependencies/integrations/forks, or complementary roles in
  the same stack. Distinguish actual integration from potential complementarity.
- business: comparable or contrasting pricing, value capture, economics or channel.
- stage: a meaningful comparison of launch/maturity/adoption signals, with dates
  where available; unknown funding is not a shared stage.

Kinds:
- fact: a source explicitly states this connection (not an independent verification).
- comparison: an analyst comparison supported by attributes of BOTH endpoints.
  Label the actual shared/different feature; it does not imply social affiliation.
- hypothesis: a plausible connection still missing decisive support; state the gap.

Use sources [{lead_id, url, excerpt?}] and a basis explaining the connection.
For comparisons, cite both endpoint records. Excerpts must be literal source text,
not invented quotes. Keep promotional/self-reported claims qualified. Set uncertain
only for hypotheses; comparisons have their own kind, not disguised factual edges.
If kind and uncertain conflict, the tool conservatively stores an uncertain
hypothesis and reports the normalized edge ids. Review the missing evidence;
do not treat normalization as verification. A submitted comparison still needs
sources for both endpoints, and malformed updates are rejected before persistence.
When useful batches arrive, add meaningful connections alongside isolated leads.
At synthesis the parent compares across platforms and checks duplicate identities;
workers only connect records in their assigned scope. Reuse known edge ids.
Prefer a few informative neighbors per entity over a clique. A shared broad topic
such as "AI" or "personal agent" alone is not an edge. Consider all dimensions,
but do not fill categories by quota. No additional acquisition solely to decorate
an edge. Missing evidence remains visible; don't manufacture social relationships.
