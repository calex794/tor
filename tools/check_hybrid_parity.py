#!/usr/bin/env python3
"""Execute unchanged pinned NEwBOO functions, emit differential Rust fixtures.

Only module loading is adapted; algorithms and their schema/coercion dependencies
are byte-verified snapshots. No network or installed NEwBOO application needed.
"""
from __future__ import annotations
import dataclasses
import hashlib
import importlib.util
import json
from pathlib import Path
import random
import sys
import types

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / 'tests/fixtures/newboo_hybrid'
SOURCE_NAMES = ['behavioral', 'content', 'popularity', 'related_books']
REASON_NAMES = ['manually_related', 'readers_also_read', 'similar_by_content',
                'trending', 'popular', 'new_release', 'fallback_popular']
TRUST_NAMES = ['normal', 'watch', 'degraded', 'reject']


def load_original():
    manifest = json.loads((FIXTURES / 'manifest.json').read_text())
    for name, expected in manifest['sha256'].items():
        assert hashlib.sha256((FIXTURES / name).read_bytes()).hexdigest() == expected, name
    for name in ['app', 'app.core', 'app.core.models', 'app.recommendations',
                 'app.recommendations.services']:
        module = types.ModuleType(name)
        module.__path__ = []
        sys.modules[name] = module
    modules = {
        'errors': 'app.core.errors', 'numeric_bounds': 'app.core.numeric_bounds',
        'ids': 'app.core.models.ids', 'type_coercion': 'app.core.type_coercion',
        'schemas': 'app.recommendations.schemas',
        'candidate_source_service': 'app.recommendations.services.candidate_source_service',
        'score_normalization_service': 'app.recommendations.services.score_normalization_service',
        'hybrid_scoring_service': 'app.recommendations.services.hybrid_scoring_service',
    }
    loaded = {}
    for file, name in modules.items():
        spec = importlib.util.spec_from_file_location(name, FIXTURES / (file + '.py'))
        module = importlib.util.module_from_spec(spec)
        sys.modules[name] = module
        spec.loader.exec_module(module)
        loaded[file] = module
    return loaded


def candidate(book='101', source='behavioral', reason='readers_also_read', score=800,
              raw=800, origin=None):
    return dict(book_id=book, source=source, reason=reason, raw_score=raw,
                normalized_score=score, source_book_id=origin)


def cases():
    oracle = json.loads((ROOT / 'docs/contracts/rec-golden-v1.json').read_text())
    for case in oracle['test_cases']:
        values = [dict(c, raw_score=c.get('raw_score', c['normalized_score']))
                  for c in case['candidates']]
        signals = [dict(book_id=book, quality_score=value, quality_multiplier_per_mille=1000,
                        trust_bucket='normal')
                   for book, value in case.get('quality_score_by_book_id', {}).items()]
        yield case['id'], list(case['weights'].values()), case['limit'], values, signals, case['expected']
    edge = [
        ('empty', [], [50,30,20], 20, []),
        ('too_many', [candidate()]*501, [50,30,20], 20, []),
        ('limit_51', [candidate()], [50,30,20], 51, []),
        ('limit_zero', [candidate()], [50,30,20], 0, []),
        ('negative_weights', [candidate()], [-1,51,50], 20, []),
        ('overflow_weights', [candidate()], [2147483647]*3, 20, []),
        ('missing', [candidate(score=None)], [50,30,20], 20, []),
        ('zero', [candidate(score=0)], [0,0,100], 20, []),
        ('related_skip', [candidate(source='related_books', score=None), candidate()], [50,30,20], 20, []),
        ('manual_skip', [candidate(reason='manually_related', score=None)], [50,30,20], 20, []),
        ('partial_rejection', [candidate(source='related_books'),candidate(),candidate(score=-1)], [50,30,20], 20, []),
        ('lexical_ids', [candidate(book='2'),candidate(book='10')], [100,0,0], 20, []),
        ('trim_ids', [candidate(book=' 101 '),candidate(book='')], [100,0,0], 20, []),
        ('popularity_cap', [candidate(book=str(i),source='popularity',reason='popular') for i in range(15)], [0,0,100], 20, []),
        ('cap_then_limit', [candidate(book=str(i),source='popularity',reason='popular',score=1000) for i in range(15)]+[candidate(book='99',score=1)], [50,0,50], 20, []),
        ('quality_50', [candidate()], [100,0,0], 20, [dict(book_id='101',quality_score=50,quality_multiplier_per_mille=1000,trust_bucket='normal')]),
        ('raw_reason_tie', [candidate(reason='popular',raw=100),candidate(reason='trending',raw=200),candidate(reason='readers_also_read',raw=200)], [100,0,0], 20, []),
    ]
    for name, values, weights, limit, quality in edge:
        yield name, weights, limit, values, quality, None
    rng = random.Random(17125)
    for index in range(160):
        b = rng.randrange(101); c = rng.randrange(101-b)
        weights = [b,c,100-b-c]
        values = [candidate(book=str(rng.randrange(1,35)), source=rng.choice(SOURCE_NAMES),
                            reason=rng.choice(REASON_NAMES), score=rng.choice([None,0,-1,1,50,800,1000,1500]),
                            raw=rng.choice([-1,0,1,500,1000,9223372036854775807]),
                            origin=rng.choice([None,'2','10'])) for _ in range(rng.randrange(1,80))]
        # Half of cases exercise accepted outputs rather than early rejection.
        if index % 2 == 0:
            for value in values:
                value['normalized_score'] = rng.randrange(1,1600)
                value['reason'] = {'behavioral':'readers_also_read','content':'similar_by_content',
                                   'popularity':rng.choice(REASON_NAMES[3:]),'related_books':'manually_related'}[value['source']]
        signals = [dict(book_id=str(rng.randrange(1,35)),quality_score=rng.choice([-100,50,1000,2000]),
                        quality_multiplier_per_mille=rng.choice([-1,500,1000,2000]),
                        trust_bucket=rng.choice(TRUST_NAMES)) for _ in range(40)]
        yield f'generated_{index}',weights,rng.randrange(1,51),values,signals,None


def main():
    modules = load_original(); schema = modules['schemas']
    score = modules['hybrid_scoring_service'].score_hybrid_candidates
    for name,weights,limit,values,signals,expected in cases():
        result = score(tuple(schema.RecommendationCandidate(**v) for v in values),
                       weights=schema.RecommendationSourceWeights(*weights),limit=limit,
                       quality_signals=tuple(types.SimpleNamespace(**s) for s in signals))
        if expected is not None:
            assert result.accepted == expected['accepted'], name
            if result.accepted:
                assert [dict(book_id=c.book_id,final_score=c.final_score) for c in result.candidates] == expected['ordered'], (name, dataclasses.asdict(result))
            else:
                assert result.rejected_reason == expected['rejected_reason'], name
        encoded = ';'.join(','.join([v['book_id'],str(SOURCE_NAMES.index(v['source'])),str(REASON_NAMES.index(v['reason'])),
                      str(v['raw_score']),'' if v['normalized_score'] is None else str(v['normalized_score']),v.get('source_book_id') or '']) for v in values)
        quality = ';'.join(','.join([s['book_id'],str(s['quality_score']),str(s['quality_multiplier_per_mille']),str(TRUST_NAMES.index(s['trust_bucket']))]) for s in signals)
        output = ';'.join(','.join([c.book_id,str(c.final_score),'.'.join(str(SOURCE_NAMES.index(s)) for s in c.sources),
                     '.'.join(str(REASON_NAMES.index(r)) for r in c.reasons),str(SOURCE_NAMES.index(c.best_source)),
                     str(REASON_NAMES.index(c.best_reason)),str(c.rank)]) for c in result.candidates)
        print('\t'.join([name,','.join(map(str,weights)),str(limit),encoded,quality,
                         result.rejected_reason or 'accepted',str(result.input_candidates_seen),
                         str(result.dedicated_candidates_skipped),output]))

if __name__ == '__main__':
    main()
