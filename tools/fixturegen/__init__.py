"""Independent fixture generators, one module per area.

Each area module defines ``AREA`` and ``cases()`` yielding :class:`model.Case`.
Generators use only the Python standard library and never import or run
Pagura, so expected results stay independent of the product.
"""
