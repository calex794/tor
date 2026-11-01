-- D-05 x D-06: additive reconciliation of the immutable RUB wallet journal.
-- Apply AFTER both the D-05 Layer schema and the merged D-06 direct refund/
-- swap schema. Never alter a previously applied SQLx migration checksum.
--
-- D-05's earlier wallet CHECK replacement omits chapter_refund/swap_credit;
-- D-06's later replacement omits layer_purchase. Preserve both contracts
-- and enforce mutually exclusive direct-purchase versus Layer provenance.
-- All accepted rows must retain the original append-only wallet triggers,
-- owner FKs, wallet_layer_term_once and d06_one_swap_wallet_credit indexes.

ALTER TABLE reader_wallet_entries
    DROP CONSTRAINT reader_wallet_entries_kind_check,
    DROP CONSTRAINT wallet_entry_direction,
    ADD CONSTRAINT reader_wallet_entries_kind_check CHECK (
        kind IN (
            'verified_funding',
            'chapter_purchase',
            'chapter_refund',
            'chapter_swap_credit',
            'layer_purchase'
        )
    ),
    ADD CONSTRAINT wallet_entry_direction CHECK (
        (kind='verified_funding'
            AND delta_minor>0
            AND purchase_id IS NULL
            AND layer_term_id IS NULL)
        OR (kind='chapter_purchase'
            AND delta_minor<0
            AND purchase_id IS NOT NULL
            AND layer_term_id IS NULL)
        OR (kind='chapter_refund'
            AND delta_minor>0
            AND purchase_id IS NOT NULL
            AND layer_term_id IS NULL)
        OR (kind='chapter_swap_credit'
            AND delta_minor>0
            AND purchase_id IS NOT NULL
            AND layer_term_id IS NULL)
        OR (kind='layer_purchase'
            AND delta_minor<0
            AND purchase_id IS NULL
            AND layer_term_id IS NOT NULL)
    );
