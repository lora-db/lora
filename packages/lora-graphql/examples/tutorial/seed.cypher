CREATE (ada:Person {key: 'ada', name: 'Ada Lovelace'}),
       (grace:Person {key: 'grace', name: 'Grace Hopper'}),
       (graphs:Tag {name: 'graphs'}),
       (history:Tag {name: 'history'}),
       (engines:Post {
         slug: 'engines', title: 'On analytical engines',
         body: 'The engine weaves algebraic patterns.', published: true
       }),
       (draft:Post {
         slug: 'notes', title: 'Notes on Bernoulli numbers',
         body: 'Unfinished.', published: false
       }),
       (cobol:Post {
         slug: 'cobol', title: 'Why COBOL reads like English',
         body: 'So that people can read it.', published: true
       }),
       (ada)-[:WROTE]->(engines),
       (ada)-[:WROTE]->(draft),
       (grace)-[:WROTE]->(cobol),
       (engines)-[:TAGGED]->(history),
       (engines)-[:TAGGED]->(graphs),
       (cobol)-[:TAGGED]->(history)
